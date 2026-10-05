//! L6 owns the complete live monthly budget view. The host supplies only
//! current plain scope identities, root and the actual L4 spend fact capability.
//! No precomputed spend or budget assessment, facade or callback enters here.
use crate::budget;
use chrono::{DateTime, Utc};
use factory_kernel::{
    resolve_scope, scope_ancestors, CostGroupBy, CostReport, CostRow, FactoryError, Facts, Provide,
    Result, ScopeIdentity, SpendQuery, L6,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

impl ScopeIdentity for Scope {
    fn scope_name(&self) -> &str {
        &self.name
    }
    fn scope_path(&self) -> &Path {
        &self.path
    }
}
pub struct Service<'a, P> {
    root: PathBuf,
    scopes: Vec<Scope>,
    root_scope: Option<Scope>,
    spend: &'a P,
}
impl<'a, P> Service<'a, P> {
    pub fn new(root: PathBuf, scopes: Vec<Scope>, root_scope: Option<Scope>, spend: &'a P) -> Self {
        Self {
            root,
            scopes,
            root_scope,
            spend,
        }
    }
}
impl<P> Service<'_, P>
where
    P: Provide<CostReport, Query = SpendQuery, Value = CostReport, Error = FactoryError>,
{
    pub async fn report(
        &self,
        scope: Option<&str>,
        group_by: CostGroupBy,
        now: DateTime<Utc>,
    ) -> Result<budget::Report> {
        let root = self.root.clone();
        let catalogue = tokio::task::spawn_blocking(move || budget::load(&root))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("budget catalogue read: {e}")))?
            .map_err(FactoryError::BadRequest)?;
        let month = budget::Month::at(now).map_err(FactoryError::BadRequest)?;
        let asked = scope
            .map(|name| resolve_scope(&self.scopes, name))
            .transpose()?;
        let mut scopes: Vec<&Scope> = self
            .scopes
            .iter()
            .filter(|candidate| {
                asked.is_none_or(|asked| {
                    candidate.path == asked.path
                        || scope_ancestors(&self.scopes, candidate)
                            .iter()
                            .any(|parent| parent.path == asked.path)
                        || (catalogue.scopes.contains_key(&candidate.id)
                            && scope_ancestors(&self.scopes, asked)
                                .iter()
                                .any(|parent| parent.path == candidate.path))
                })
            })
            .collect();
        if let Some(asked) = asked {
            if !scopes.iter().any(|s| s.id == asked.id) {
                scopes.push(asked);
            }
        } else if let Some(root) = self.root_scope.as_ref() {
            if !scopes.iter().any(|s| s.id == root.id) {
                scopes.push(root);
            }
        }
        scopes.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.id.cmp(&b.id)));
        let spend = self
            .month_spend(asked.map(|s| s.name.clone()), group_by, &month)
            .await?;
        let mut budgets = Vec::new();
        for target in scopes {
            let scoped =
                if spend.scope.as_deref() == Some(&target.name) && group_by == CostGroupBy::Scope {
                    spend.clone()
                } else {
                    self.month_spend(Some(target.name.clone()), CostGroupBy::Scope, &month)
                        .await?
                };
            let limit = catalogue.scopes.get(&target.id).map(|b| b.monthly_usd);
            let relation = match asked {
                Some(asked) if target.path == asked.path => "selected",
                Some(asked)
                    if scope_ancestors(&self.scopes, asked)
                        .iter()
                        .any(|parent| parent.path == target.path) =>
                {
                    "ancestor"
                }
                Some(_) => "descendant",
                None => "instance",
            };
            budgets.push(budget::ScopeBudget {
                id: target.id.clone(),
                scope: target.name.clone(),
                path: target.path.display().to_string(),
                relation: relation.into(),
                monthly_usd: limit,
                assessment: budget::assess(limit, &scoped.total, scoped.unattributed_runs, &month),
                spent: scoped.total,
                unattributed_runs: scoped.unattributed_runs,
                daily: scoped.daily,
            });
        }
        let findings = catalogue.scopes.keys().filter(|id| !self.scopes.iter().any(|s| &s.id == *id)
            && !self.root_scope.as_ref().is_some_and(|s| &s.id == *id)
            && !asked.is_some_and(|s| &s.id == *id))
            .map(|id| format!("Budget scope id {id:?} is not a configured scope; authored intent was preserved")).collect();
        Ok(budget::Report {
            catalogue: budget::catalogue_path(&self.root).display().to_string(),
            month,
            group_by,
            spend,
            budgets,
            findings,
        })
    }

    /// At the exact first instant of a month the logical window is empty.
    /// This is known empty spend, not a failed/missing observation.
    pub async fn month_spend(
        &self,
        scope: Option<String>,
        group_by: CostGroupBy,
        month: &budget::Month,
    ) -> Result<CostReport> {
        if month.from == month.as_of {
            return Ok(CostReport {
                basis: factory_kernel::SpendBasis::Started,
                finished: None,
                group_by,
                from: month.from,
                to: month.as_of,
                scope,
                rows: Vec::new(),
                total: CostRow::new("total", None),
                unattributed_runs: 0,
                daily: Vec::new(),
            });
        }
        Facts::<L6>::new()
            .get::<CostReport, _>(
                self.spend,
                &SpendQuery {
                    scope,
                    from: Some(month.from),
                    to: Some(month.as_of),
                    group_by,
                    ..Default::default()
                },
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_kernel::{FactProvider, SpendBasis, L4};
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    };

    #[derive(Default)]
    struct Spend {
        calls: Mutex<Vec<SpendQuery>>,
        value: AtomicUsize,
        fail: AtomicBool,
    }
    impl FactProvider for Spend {
        type Level = L4;
    }
    #[async_trait::async_trait]
    impl Provide<CostReport> for Spend {
        type Query = SpendQuery;
        type Value = CostReport;
        type Error = FactoryError;
        async fn get(&self, query: &SpendQuery) -> Result<CostReport> {
            self.calls.lock().unwrap().push(query.clone());
            if self.fail.load(Ordering::SeqCst) {
                return Err(FactoryError::BadRequest("spend unavailable".into()));
            }
            assert_eq!(query.basis, SpendBasis::Started);
            Ok(CostReport {
                basis: query.basis,
                finished: None,
                group_by: query.group_by,
                from: query.from.unwrap(),
                to: query.to.unwrap(),
                scope: query.scope.clone(),
                rows: Vec::new(),
                total: CostRow {
                    runs: 1,
                    cost_usd: self.value.load(Ordering::SeqCst) as f64,
                    ..Default::default()
                },
                unattributed_runs: 0,
                daily: Vec::new(),
            })
        }
    }
    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("factory-budget-owner-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(budget::budgets_dir(&path)).unwrap();
            Self(path)
        }
        fn limits(&self, raw: &str) {
            std::fs::write(budget::catalogue_path(&self.0), raw).unwrap();
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn scope(id: &str, name: &str, path: &str) -> Scope {
        Scope {
            id: id.into(),
            name: name.into(),
            path: path.into(),
        }
    }
    fn scopes() -> Vec<Scope> {
        vec![
            scope("root", "company", "."),
            scope("work", "work", "projects/work"),
            scope("child", "child", "projects/work/deep"),
            scope("fake", "work/fake", "projects/work-other"),
        ]
    }
    fn now() -> DateTime<Utc> {
        "2026-10-16T12:00:00Z".parse().unwrap()
    }
    fn service<'a>(root: &Root, spend: &'a Spend) -> Service<'a, Spend> {
        Service::new(
            root.0.clone(),
            scopes(),
            Some(scope("root", "company", ".")),
            spend,
        )
    }

    #[tokio::test]
    async fn live_budget_reads_exact_scope_subtrees_and_independent_ancestor_caps() {
        let root = Root::new();
        let spend = Spend::default();
        root.limits("version: 1\nscopes: {root: {monthly_usd: 50}, work: {monthly_usd: 10}, child: {monthly_usd: 20}}\n");
        spend.value.store(3, Ordering::SeqCst);
        let report = service(&root, &spend)
            .report(Some("projects/work"), CostGroupBy::Scope, now())
            .await
            .unwrap();
        assert_eq!(report.spend.scope.as_deref(), Some("work"));
        assert_eq!(
            report.spend.total.cost_usd, 3.0,
            "overlapping budgets never sum into spend"
        );
        assert_eq!(
            report
                .budgets
                .iter()
                .map(|b| (b.scope.as_str(), b.relation.as_str()))
                .collect::<Vec<_>>(),
            [
                ("company", "ancestor"),
                ("work", "selected"),
                ("child", "descendant")
            ]
        );
        assert_eq!(
            report
                .budgets
                .iter()
                .map(|b| b.monthly_usd)
                .collect::<Vec<_>>(),
            [Some(50.0), Some(10.0), Some(20.0)]
        );
        let calls = spend.calls.lock().unwrap();
        assert_eq!(
            calls.iter().map(|q| q.scope.as_deref()).collect::<Vec<_>>(),
            [Some("work"), Some("company"), Some("child")]
        );
        for q in calls.iter() {
            assert_eq!(q.from, Some(report.month.from));
            assert_eq!(q.to, Some(report.month.as_of));
        }
    }

    #[tokio::test]
    async fn authored_intent_and_lower_spend_are_reread_without_a_status_cache() {
        let root = Root::new();
        let spend = Spend::default();
        root.limits("version: 1\nscopes: {work: {monthly_usd: 10}, removed: {monthly_usd: 90}}\n");
        let owner = service(&root, &spend);
        let first = owner
            .report(Some("work"), CostGroupBy::Scope, now())
            .await
            .unwrap();
        assert_eq!(first.budgets[0].monthly_usd, Some(10.0));
        assert_eq!(first.findings, ["Budget scope id \"removed\" is not a configured scope; authored intent was preserved"]);
        root.limits("version: 1\nscopes: {work: {monthly_usd: 20}}\n");
        spend.value.store(7, Ordering::SeqCst);
        let next = owner
            .report(Some("work"), CostGroupBy::Scope, now())
            .await
            .unwrap();
        assert_eq!(next.budgets[0].monthly_usd, Some(20.0));
        assert_eq!(next.spend.total.cost_usd, 7.0);
        assert!(next.findings.is_empty());
        spend.fail.store(true, Ordering::SeqCst);
        assert!(owner
            .report(Some("work"), CostGroupBy::Scope, now())
            .await
            .unwrap_err()
            .to_string()
            .contains("spend unavailable"));
        spend.fail.store(false, Ordering::SeqCst);
        assert!(owner
            .report(Some("work"), CostGroupBy::Scope, now())
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn exact_month_start_is_known_empty_and_never_calls_even_a_failing_provider() {
        let root = Root::new();
        let spend = Spend::default();
        spend.fail.store(true, Ordering::SeqCst);
        root.limits("version: 1\nscopes: {work: {monthly_usd: 0}}\n");
        let midnight = "2026-10-01T00:00:00Z".parse().unwrap();
        for group in [
            CostGroupBy::Scope,
            CostGroupBy::Task,
            CostGroupBy::Agent,
            CostGroupBy::Issue,
            CostGroupBy::Provider,
            CostGroupBy::Workflow,
        ] {
            let report = service(&root, &spend)
                .report(Some("work"), group, midnight)
                .await
                .unwrap();
            assert_eq!(report.spend.group_by, group);
            assert_eq!(report.spend.from, report.spend.to);
            assert_eq!(report.spend.total, CostRow::new("total", None));
            assert_eq!(report.spend.unattributed_runs, 0);
            assert!(report.spend.rows.is_empty() && report.spend.daily.is_empty());
        }
        assert!(spend.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn invalid_intent_and_unknown_or_ambiguous_scopes_fail_before_spend_is_read() {
        let root = Root::new();
        let spend = Spend::default();
        root.limits("version: 1\nscopes: {work: {monthly_usd: -1}}\n");
        assert!(service(&root, &spend)
            .report(Some("work"), CostGroupBy::Scope, now())
            .await
            .is_err());
        root.limits("version: 1\nscopes: {}\n");
        assert!(matches!(
            service(&root, &spend)
                .report(Some("missing"), CostGroupBy::Scope, now())
                .await,
            Err(FactoryError::NoSuchScope(_))
        ));
        let owner = Service::new(
            root.0.clone(),
            vec![
                scope("a", "a/work", "a/work"),
                scope("b", "b/work", "b/work"),
            ],
            None,
            &spend,
        );
        assert!(owner
            .report(Some("work"), CostGroupBy::Scope, now())
            .await
            .unwrap_err()
            .to_string()
            .contains("could mean any of"));
        assert!(spend.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unregistered_root_card_and_grouped_spend_preserve_existing_selection_rules() {
        let root = Root::new();
        let spend = Spend::default();
        root.limits("version: 1\nscopes: {root-only: {monthly_usd: 50}}\n");
        let owner = Service::new(
            root.0.clone(),
            vec![scope("work", "work", "projects/work")],
            Some(scope("root-only", "company", ".")),
            &spend,
        );
        let all = owner.report(None, CostGroupBy::Agent, now()).await.unwrap();
        assert_eq!(all.spend.scope, None);
        assert_eq!(all.spend.group_by, CostGroupBy::Agent);
        assert_eq!(
            all.budgets
                .iter()
                .map(|b| b.id.as_str())
                .collect::<Vec<_>>(),
            ["root-only", "work"]
        );
        assert!(all.findings.is_empty());
        let calls = spend.calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .map(|q| (q.scope.as_deref(), q.group_by))
                .collect::<Vec<_>>(),
            [
                (None, CostGroupBy::Agent),
                (Some("company"), CostGroupBy::Scope),
                (Some("work"), CostGroupBy::Scope)
            ]
        );
    }
}
