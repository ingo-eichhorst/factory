//! The L5 Metrics tab's listing (`#333`): a page. It composes L6's
//! applicable policy subjects and current key results, L5's own quality
//! trees and reported declarations, and one ordinary metrics read, into the
//! rows `factory_assurance::metrics_catalogue` shapes. No state of its own;
//! every input is read fresh, and the values come from `metrics_for`, so a
//! service-backed source is still asked once per listing.
use crate::engine::Engine;
use crate::metrics::{reported_configuration, Metrics};
use chrono::{DateTime, Utc};
use factory_assurance::metrics_catalogue::{self, MetricRow};
use factory_core::{
    error::{FactoryError, Result},
    metrics::MetricsWindow,
};
use std::{collections::BTreeSet, path::Component};

impl Engine {
    /// Every metric `scope`'s subtree is measured by, with each value's
    /// source and every consumer's verdict. Key results count only when
    /// `scope` is the instance root: the goals catalogue is the company's.
    pub(crate) async fn metrics_listing(
        &self,
        scope: &str,
        now: DateTime<Utc>,
        window: Option<MetricsWindow>,
    ) -> Result<(Metrics, Vec<MetricRow>)> {
        let snapshot = self.factory_snapshot();
        let tree = snapshot.scope_tree();
        let (asked, targets) = tree.subtree_scopes(Some(scope))?;
        let asked = asked.ok_or_else(|| FactoryError::NoSuchScope(scope.to_string()))?;
        let canonical = asked.name.clone();
        let at_root = asked.path.components().all(|c| matches!(c, Component::CurDir));
        let members: BTreeSet<String> = targets.iter().map(|s| s.name.clone()).collect();

        let policy = self
            .l6_service()
            .metric_policy_inputs(&snapshot, Some(&canonical))
            .await?;
        let mut readers = metrics_catalogue::policy_readers(&policy);
        let quality = crate::quality::quality_configuration(&snapshot)
            .read(snapshot.root.clone(), Some(&canonical), false)
            .await?;
        readers.extend(metrics_catalogue::quality_readers(&quality));
        if at_root {
            let goals_dir = factory_core::goals::goals_dir(&snapshot.root);
            let catalogue = tokio::task::spawn_blocking(move || factory_core::goals::load(&goals_dir))
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("goals walk: {e}")))?;
            let key_results = factory_direction::goals_service::metric_key_results(&catalogue, now);
            readers.extend(metrics_catalogue::goal_readers(&key_results, Some(&canonical)));
        }

        let catalogue =
            factory_assurance::reported::validate(&snapshot.root, &reported_configuration(&snapshot));
        let ids = metrics_catalogue::listed_ids(&readers, &catalogue, &members);
        let metrics = if ids.is_empty() {
            Metrics {
                values: Vec::new(),
                series: Vec::new(),
                registry: Vec::new(),
                findings: catalogue.findings.clone(),
            }
        } else {
            self.metrics_for(&ids, now, Some(&canonical), window).await?
        };
        let rows = metrics_catalogue::rows(
            &metrics.values,
            &readers,
            &catalogue,
            &tree,
            &snapshot.root,
            now,
        )?;
        Ok((metrics, rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_assurance::metrics_catalogue::{ConsumerKind, ConsumerStatus, Freshness, MetricSource};
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, PolicyDeclaration, Scope};
    use factory_plugins::{Registry, SqliteStore};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A local stand-in for a scope's metrics service: answers every GET
    /// with whatever `body` currently holds, and counts the requests.
    struct Served {
        port: u16,
        hits: Arc<AtomicUsize>,
        body: Arc<std::sync::Mutex<String>>,
    }

    impl Served {
        async fn start() -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let hits = Arc::new(AtomicUsize::new(0));
            let body = Arc::new(std::sync::Mutex::new(String::new()));
            let (counter, served) = (hits.clone(), body.clone());
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let body = served.lock().unwrap().clone();
                    tokio::spawn(async move {
                        let mut head = Vec::new();
                        let mut byte = [0u8; 1];
                        while !head.ends_with(b"\r\n\r\n") {
                            match stream.read(&mut byte).await {
                                Ok(1) => head.push(byte[0]),
                                _ => return,
                            }
                        }
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    });
                }
            });
            Self { port, hits, body }
        }

        fn answer(&self, value: f64, as_of: DateTime<Utc>) {
            *self.body.lock().unwrap() = format!(
                r#"{{"as_of":"{}","metrics":[{{"id":"bookings_without_beleg","value":{value}}}]}}"#,
                as_of.to_rfc3339()
            );
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    fn scope(yaml: &str, path: &str) -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(yaml).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    const GOBD: &str = "framework: gobd\ntitle: GoBD\nkind: regulation\ncontrols:\n\
        \x20\x20- id: belegprinzip\n    title: Belegprinzip\n    evidence:\n\
        \x20\x20\x20\x20\x20\x20- check: metric\n        metric: reported.finance.bookings_without_beleg\n        below: 0\n";

    /// `company` (the instance root) over `finance` and `ops`. `finance`
    /// alone commits to GoBD (and, with `quality`, binds the `books`
    /// profile), and declares the one reported metric, read from `source` --
    /// a file block or a service block -- with `fava` declared as its
    /// dependency service at `endpoints`.
    fn engine(source: &str, endpoints: &[String], quality: bool) -> (Arc<Engine>, PathBuf) {
        let root = std::env::temp_dir().join(format!("factory-metrics-page-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory/policies")).unwrap();
        std::fs::create_dir_all(root.join(".factory/quality")).unwrap();
        std::fs::create_dir_all(root.join("projects/finance")).unwrap();
        std::fs::write(root.join(".factory/policies/gobd.yaml"), GOBD).unwrap();
        std::fs::write(
            root.join(".factory/quality/books.yaml"),
            "attributes:\n  - id: functional-suitability.functional-correctness\n    importance: H\n    difficulty: M\n    scenarios:\n\
             \x20\x20\x20\x20\x20\x20- { id: beleg, measure: { metric: reported.finance.bookings_without_beleg, below: 0, max_age: 35d } }\n",
        )
        .unwrap();
        let endpoints = endpoints.iter().map(|e| format!("\"{e}\"")).collect::<Vec<_>>().join(", ");
        let quality = if quality { "books" } else { "" };
        let company = scope("id: company-id\nname: company\n", ".");
        let finance = scope(
            &format!(
                "id: finance-id\nname: finance\npolicies: {{ frameworks: [gobd] }}\nquality: [{quality}]\n\
                 dependencies:\n  services:\n    - {{ name: fava, transport: network, endpoints: [{endpoints}] }}\n\
                 metrics:\n  source: {source}\n  declare:\n\
                 \x20\x20\x20\x20- {{ id: bookings_without_beleg, title: Bookings without Beleg, unit: count, better: lower }}\n"
            ),
            "projects/finance",
        );
        let ops = scope("id: ops-id\nname: ops\n", "projects/ops");
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(company.clone()),
            scopes: vec![company, finance, ops],
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration::default(),
            quality: Vec::new(),
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        let engine = Arc::new(Engine::new(
            Factory { root: root.clone(), config },
            Registry::with_builtins(),
            store,
            PathBuf::from("factory"),
            Vec::new(),
        ));
        (engine, root)
    }

    fn write_goals(root: &Path, metric: &str) {
        let dir = factory_core::goals::goals_dir(root);
        std::fs::create_dir_all(&dir).unwrap();
        let today = Utc::now().date_naive();
        std::fs::write(
            dir.join("now.yaml"),
            format!(
                "cycle: {{ id: now, from: {}, to: {} }}\nobjectives:\n\
                 \x20\x20- id: books\n    title: Clean books\n    key_results:\n\
                 \x20\x20\x20\x20\x20\x20- {{ id: beleg, title: Every booking has a Beleg, kind: committed, metric: {metric}, baseline: 10, target: 0 }}\n",
                today - chrono::Duration::days(10),
                today + chrono::Duration::days(30),
            ),
        )
        .unwrap();
    }

    const METRIC: &str = "reported.finance.bookings_without_beleg";

    /// `#333`'s acceptance criterion: a scope with one `metric` control
    /// lists one row, with that control as its consumer and its verdict.
    #[tokio::test]
    async fn a_scope_with_one_metric_control_lists_one_row_with_its_consumer_and_verdict() {
        let (engine, root) = engine("{ id: finance, file: metrics.json }", &["127.0.0.1:1".into()], false);
        let now = Utc::now();
        std::fs::write(
            root.join("projects/finance/metrics.json"),
            format!(r#"{{"as_of":"{}","metrics":[{{"id":"bookings_without_beleg","value":2}}]}}"#, now.to_rfc3339()),
        )
        .unwrap();

        let (metrics, rows) = engine.metrics_listing("finance", now, None).await.unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(metrics.values.len(), 1);
        let row = &rows[0];
        assert_eq!(row.id.as_str(), METRIC);
        assert_eq!(metrics.values[0].value, Some(2.0));
        assert!(matches!(&row.source, MetricSource::File { path, .. } if path == "projects/finance/metrics.json"));
        assert_eq!(row.consumers.len(), 1, "{:?}", row.consumers);
        let consumer = &row.consumers[0];
        assert_eq!((consumer.kind, consumer.id.as_str()), (ConsumerKind::Policy, "gobd/belegprinzip"));
        assert_eq!(consumer.scope.as_deref(), Some("finance"));
        assert_eq!(consumer.status, ConsumerStatus::NotMet, "2 bookings without a Beleg is above the allowed 0");
        let def = metrics.registry.iter().find(|d| d.id == METRIC).unwrap();
        assert_eq!(def.title, "Bookings without Beleg");

        // A sibling nothing measures lists nothing at all.
        let (_, ops) = engine.metrics_listing("ops", now, None).await.unwrap();
        assert!(ops.is_empty(), "{ops:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// A policy `metric` check, a quality measure and a key result read a
    /// service-backed metric exactly as they read a file-backed one -- and
    /// each read asks the service once, however many readers it has.
    #[tokio::test]
    async fn policy_quality_and_goals_read_a_service_backed_metric_like_a_file_backed_one() {
        let served = Served::start().await;
        let endpoints = [
            "https://factory.tail1234.ts.net:8793/buchhaltung".to_string(),
            format!("127.0.0.1:{}", served.port),
        ];
        let (engine, root) = engine("{ id: finance, service: fava, path: /metrics }", &endpoints, true);
        write_goals(&root, METRIC);
        served.answer(0.0, Utc::now());

        let before = served.hits();
        let report = engine.policy_report(Some("finance")).await.unwrap();
        let status = report.rows[0]
            .statuses
            .iter()
            .find(|s| s.control.id == "belegprinzip")
            .unwrap()
            .status
            .kind();
        assert_eq!(status, factory_core::policy::StatusKind::Satisfied);
        assert_eq!(served.hits() - before, 1, "one request for the Policy read");

        let before = served.hits();
        let quality = engine.quality_report(Some("finance")).await.unwrap();
        let scenario = quality
            .scopes
            .iter()
            .flat_map(|s| &s.report.attributes)
            .flat_map(|a| &a.scenarios)
            .find(|s| s.scenario.scenario.id == "beleg")
            .unwrap();
        assert_eq!(scenario.status, factory_core::quality::ScenarioStatus::Met);
        assert_eq!(served.hits() - before, 1, "one request for the Quality read");

        let before = served.hits();
        let goals = engine.goals_report(None, None).await.unwrap();
        let kr = &goals.report.as_ref().unwrap().objectives[0].key_results[0];
        assert_eq!(kr.value, Some(0.0));
        assert_eq!(served.hits() - before, 1, "one request for the Goals read");

        // compliance.gobd's own nested policy read reuses the document the
        // outer read already holds.
        let before = served.hits();
        let ids = [
            factory_core::metrics::MetricId::new("compliance.gobd").unwrap(),
            factory_core::metrics::MetricId::new(METRIC).unwrap(),
        ];
        let computed = engine.metrics_for(&ids, Utc::now(), Some("finance"), None).await.unwrap();
        assert_eq!(computed.values[0].value, Some(1.0), "{:?}", computed.values);
        assert_eq!(served.hits() - before, 1, "one request for a metrics read that also rolls up policy");

        let before = served.hits();
        let (_, rows) = engine.metrics_listing("company", Utc::now(), None).await.unwrap();
        assert_eq!(served.hits() - before, 1, "one request for the listing");
        assert_eq!(rows.len(), 1, "{rows:?}");
        let row = &rows[0];
        assert!(matches!(
            &row.source,
            MetricSource::Service { service, endpoint, url, .. }
                if service == "fava" && *endpoint == endpoints[1] && url.ends_with("/metrics")
        ));
        assert_eq!(row.freshness, Freshness::Fresh);
        assert_eq!(row.max_age.unwrap().as_hours(), 35 * 24, "the quality scenario's window is the strictest");
        let kinds: Vec<_> = row.consumers.iter().map(|c| (c.kind, c.scope.clone().unwrap_or_default())).collect();
        assert!(kinds.contains(&(ConsumerKind::Policy, "finance".into())), "{kinds:?}");
        assert!(kinds.contains(&(ConsumerKind::Quality, "finance".into())), "{kinds:?}");
        assert!(kinds.contains(&(ConsumerKind::Goal, "company".into())), "{kinds:?}");
        assert!(row.consumers.iter().all(|c| c.status == ConsumerStatus::Met), "{:?}", row.consumers);

        // Key results are the instance root's: a child scope's listing has none.
        let (_, rows) = engine.metrics_listing("finance", Utc::now(), None).await.unwrap();
        assert!(rows[0].consumers.iter().all(|c| c.kind != ConsumerKind::Goal));
        std::fs::remove_dir_all(&root).ok();
    }

    /// A service with only a tailnet endpoint is refused before anything is
    /// asked: a finding, and the metric reads `None` with the reason.
    #[tokio::test]
    async fn a_tailnet_only_service_reads_none_with_a_finding_and_never_fails_the_listing() {
        let (engine, root) = engine(
            "{ id: finance, service: fava, path: /metrics }",
            &["https://factory.tail1234.ts.net:8793/buchhaltung".into()],
            false,
        );
        let (metrics, rows) = engine.metrics_listing("finance", Utc::now(), None).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(metrics.values[0].value, None);
        assert!(
            metrics.values[0].reason.as_deref().unwrap().contains("no loopback endpoint"),
            "{:?}",
            metrics.values[0].reason
        );
        assert!(matches!(&rows[0].source, MetricSource::Unresolved { service: Some(s), .. } if s == "fava"));
        assert_eq!(rows[0].consumers[0].status, ConsumerStatus::NoData);
        assert!(metrics
            .findings
            .iter()
            .any(|f| f.kind == factory_assurance::reported::FindingKind::NoLoopbackEndpoint));
        std::fs::remove_dir_all(&root).ok();
    }
}
