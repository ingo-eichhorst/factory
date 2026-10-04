//! L6 policy remediation and scenario promotion command rules.
//! This owner can command only L5 and reads process inventory through L0 facts.
use crate::{policy, policy_report::PolicyControlDetail};
use factory_assurance::remediation::{Intent, RemediationCommands};
use factory_kernel::{
    CommandPort, Commands, FactoryError, Facts, Provide, Result, TaskInventoryFact,
    TaskInventoryQuery, TaskReceipt, L6,
};
use std::collections::BTreeMap;

pub struct Service<P: RemediationCommands, I> {
    pub assurance: Commands<L6, P>,
    pub inventory: I,
}
impl<P: RemediationCommands, I> CommandPort for Service<P, I> {
    type Level = L6;
}
pub struct PromotedReceipt {
    pub control: policy::ControlRef,
    pub task: TaskReceipt,
}
pub struct SkippedReceipt {
    pub control: policy::ControlRef,
    pub existing_task: String,
}
pub struct PromotionReceipt {
    pub scenario: String,
    pub scope: String,
    pub created: Vec<PromotedReceipt>,
    pub skipped: Vec<SkippedReceipt>,
}

impl<
        P: RemediationCommands,
        I: Provide<
            TaskInventoryFact,
            Query = TaskInventoryQuery,
            Value = Vec<TaskInventoryFact>,
            Error = FactoryError,
        >,
    > Service<P, I>
{
    async fn inventory(&self, scope: &str) -> Result<Vec<TaskInventoryFact>> {
        Facts::<L6>::new()
            .get::<TaskInventoryFact, _>(
                &self.inventory,
                &TaskInventoryQuery::Exact(scope.to_string()),
            )
            .await
    }
    pub async fn policy(
        &self,
        control: policy::ControlRef,
        scope: &str,
        agent: Option<String>,
        detail: &PolicyControlDetail,
    ) -> Result<TaskReceipt> {
        match detail.status.kind() {
            policy::StatusKind::Satisfied => {
                return Err(FactoryError::BadRequest(format!(
                    "{control} is already satisfied at {scope:?}; nothing to remediate"
                )));
            }
            policy::StatusKind::Attested => {
                return Err(FactoryError::BadRequest(format!(
                    "{control} is already attested at {scope:?}; nothing to remediate"
                )));
            }
            policy::StatusKind::NotApplicable => {
                let rationale = detail
                    .not_applicable
                    .as_ref()
                    .map(|na| na.rationale.as_str())
                    .unwrap_or("");
                return Err(FactoryError::BadRequest(format!(
                    "{control} is not applicable at {scope:?}: {rationale}"
                )));
            }
            policy::StatusKind::Open | policy::StatusKind::Stale => {}
        }

        if let Some(task) = self
            .inventory(scope)
            .await?
            .into_iter()
            .find(|t| open_policy_label(t) == Some(control.to_string().as_str()))
        {
            return Err(FactoryError::BadRequest(format!(
                "a task to close {control} at {scope:?} is already open: {} ({:?})",
                task.id, task.title
            )));
        }

        let mut labels = BTreeMap::new();
        labels.insert("policy".to_string(), control.to_string());
        let intent = Intent {
            title: format!("Close {control}: {}", detail.title),
            instructions: policy::remediation_instructions(
                &control,
                detail.remediation.as_deref(),
                &detail.status,
                &detail.checks,
            ),
            scope: scope.to_string(),
            agent,
            labels,
        };
        self.assurance.port().remediate(intent).await
    }
    pub async fn promote(
        &self,
        scenario_name: &str,
        scope: &str,
        agent: Option<String>,
        scenario_applied: &[policy::Applied],
        scenario_statuses: &[policy::ControlStatus],
        delta: &crate::scenario::PolicyDelta,
    ) -> Result<PromotionReceipt> {
        let existing_tasks = self.inventory(scope).await?;
        let mut created = Vec::new();
        let mut skipped = Vec::new();
        for control in &delta.newly_open {
            let label = control.to_string();
            if let Some(existing) = existing_tasks
                .iter()
                .find(|t| open_policy_label(t) == Some(label.as_str()))
            {
                skipped.push(SkippedReceipt {
                    control: control.clone(),
                    existing_task: existing.id.clone(),
                });
                continue;
            }

            let applied_entry = scenario_applied
                .iter()
                .find(|a| &a.control == control)
                .expect("a newly_open control is always in the scenario's own applied set");
            let status_entry = scenario_statuses
                .iter()
                .find(|status| &status.control == control)
                .expect("a newly_open control is always in the scenario's own evaluated statuses");

            let mut labels = BTreeMap::new();
            labels.insert("policy".to_string(), label);
            labels.insert("scenario".to_string(), scenario_name.to_string());
            let intent = Intent {
                title: format!(
                    "Prepare {control} for scenario {}: {}",
                    scenario_name, applied_entry.title
                ),
                instructions: policy::remediation_instructions(
                    control,
                    applied_entry.remediation.as_deref(),
                    &status_entry.status,
                    &applied_entry.evidence,
                ),
                scope: scope.to_string(),
                agent: agent.clone(),
                labels,
            };
            let task = self.assurance.port().remediate(intent).await?;
            created.push(PromotedReceipt {
                control: control.clone(),
                task,
            });
        }

        Ok(PromotionReceipt {
            scenario: scenario_name.to_string(),
            scope: scope.to_string(),
            created,
            skipped,
        })
    }
}
pub fn open_policy_label(task: &TaskInventoryFact) -> Option<&str> {
    if !task.open {
        return None;
    }
    task.labels.get("policy").map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct Assurance {
        requests: Mutex<Vec<Intent>>,
        fail_at: Option<usize>,
    }
    impl CommandPort for Assurance {
        type Level = factory_kernel::L5;
    }
    #[async_trait::async_trait]
    impl RemediationCommands for Assurance {
        async fn remediate(&self, intent: Intent) -> Result<TaskReceipt> {
            let mut requests = self.requests.lock().unwrap();
            requests.push(intent);
            if self.fail_at == Some(requests.len()) {
                return Err(FactoryError::BadRequest("lower refused".into()));
            }
            Ok(TaskReceipt {
                id: format!("created-{}", requests.len()),
            })
        }
    }
    struct Inventory {
        rows: Mutex<Vec<TaskInventoryFact>>,
        reads: Mutex<Vec<String>>,
    }
    impl factory_kernel::FactProvider for Inventory {
        type Level = factory_kernel::L4;
    }
    #[async_trait::async_trait]
    impl Provide<TaskInventoryFact> for Inventory {
        type Query = TaskInventoryQuery;
        type Value = Vec<TaskInventoryFact>;
        type Error = FactoryError;
        async fn get(&self, q: &TaskInventoryQuery) -> Result<Self::Value> {
            let TaskInventoryQuery::Exact(scope) = q else {
                panic!("exact inventory required")
            };
            self.reads.lock().unwrap().push(scope.clone());
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|t| &t.scope == scope)
                .cloned()
                .collect())
        }
    }
    fn service(fail_at: Option<usize>) -> Service<Assurance, Inventory> {
        Service {
            assurance: Commands::new(Assurance {
                requests: Mutex::new(vec![]),
                fail_at,
            }),
            inventory: Inventory {
                rows: Mutex::new(vec![]),
                reads: Mutex::new(vec![]),
            },
        }
    }
    fn detail(status: policy::Status) -> PolicyControlDetail {
        serde_json::from_value(serde_json::json!({
            "control":"house/restore","title":"Restore backup","kind":"best-practice","checks":[{"check":"task","task":"restore"}],"maps_to":[],
            "remediation":"Run a restore.","status":status,"attestations":[]
        })).unwrap()
    }
    fn row(id: &str, scope: &str, open: bool) -> TaskInventoryFact {
        TaskInventoryFact {
            id: id.into(),
            title: format!("Title {id}"),
            scope: scope.into(),
            open,
            labels: BTreeMap::from([("policy".into(), "house/restore".into())]),
        }
    }
    #[tokio::test]
    async fn policy_refuses_covered_controls_before_fact_or_command_access() {
        let s = service(None);
        for (status, why) in [
            (
                policy::Status::Satisfied { reasons: vec![] },
                "already satisfied",
            ),
            (
                policy::Status::Attested { reasons: vec![] },
                "already attested",
            ),
            (
                policy::Status::NotApplicable { reasons: vec![] },
                "not applicable",
            ),
        ] {
            let mut d = detail(status);
            d.not_applicable = Some(policy::AppliedNotApplicable {
                scope: "demo".into(),
                rationale: "Only hosts".into(),
            });
            let error = s
                .policy(d.control.clone(), "demo", None, &d)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(why));
            if why == "not applicable" {
                assert!(error.contains("Only hosts"));
            }
        }
        assert!(s.inventory.reads.lock().unwrap().is_empty());
        assert!(s.assurance.port().requests.lock().unwrap().is_empty());
    }
    #[tokio::test]
    async fn policy_uses_live_exact_facts_for_duplicates_then_commands_only_l5() {
        let s = service(None);
        let d = detail(policy::Status::Open {
            reasons: vec!["Missing restore".into()],
        });
        *s.inventory.rows.lock().unwrap() = vec![
            row("other", "child", true),
            row("done", "demo", false),
            row("newest", "demo", true),
            row("older", "demo", true),
        ];
        let error = s
            .policy(d.control.clone(), "demo", None, &d)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("newest") && error.contains("Title newest") && !error.contains("older")
        );
        assert!(s.assurance.port().requests.lock().unwrap().is_empty());
        for r in s.inventory.rows.lock().unwrap().iter_mut() {
            r.open = false;
        }
        let receipt = s
            .policy(d.control.clone(), "demo", Some("worker".into()), &d)
            .await
            .unwrap();
        assert_eq!(receipt.id, "created-1");
        let requests = s.assurance.port().requests.lock().unwrap();
        let intent = &requests[0];
        assert_eq!(intent.title, "Close house/restore: Restore backup");
        assert_eq!(
            intent.instructions,
            policy::remediation_instructions(
                &d.control,
                d.remediation.as_deref(),
                &d.status,
                &d.checks
            )
        );
        assert_eq!(intent.scope, "demo");
        assert_eq!(intent.agent, Some("worker".into()));
        assert_eq!(intent.labels["policy"], "house/restore");
        assert_eq!(*s.inventory.reads.lock().unwrap(), vec!["demo", "demo"]);
    }
    #[tokio::test]
    async fn promotion_keeps_order_skips_open_tasks_and_propagates_partial_submission_failure() {
        let make = |name: &str| {
            let mut d = detail(policy::Status::Open { reasons: vec![] });
            d.control = policy::ControlRef::new("house", name);
            policy::Applied {
                control: d.control,
                title: name.into(),
                kind: d.kind,
                maps_to: vec![],
                evidence: d.checks,
                max_age: None,
                not_applicable: None,
                remediation: d.remediation,
                requires: vec![],
            }
        };
        let applied = vec![make("first"), make("restore"), make("last")];
        let statuses = policy::evaluate(&applied, &policy::Evidence::default(), chrono::Utc::now());
        let delta = crate::scenario::policy_delta(&[], &statuses);
        let s = service(None);
        *s.inventory.rows.lock().unwrap() = vec![row("existing", "demo", true)];
        let receipt = s
            .promote(
                "future",
                "demo",
                Some("worker".into()),
                &applied,
                &statuses,
                &delta,
            )
            .await
            .unwrap();
        assert_eq!(
            receipt
                .created
                .iter()
                .map(|c| c.control.to_string())
                .collect::<Vec<_>>(),
            vec!["house/first", "house/last"]
        );
        assert_eq!(receipt.skipped[0].existing_task, "existing");
        let requests = s.assurance.port().requests.lock().unwrap();
        assert_eq!(
            requests[0].title,
            "Prepare house/first for scenario future: first"
        );
        assert_eq!(
            requests[1].title,
            "Prepare house/last for scenario future: last"
        );
        assert_eq!(requests[0].labels["scenario"], "future");
        assert_eq!(requests[1].labels["policy"], "house/last");
        assert_eq!(requests[0].agent, Some("worker".into()));
        assert_eq!(requests[0].scope, "demo");
        let failing = service(Some(2));
        let error = failing
            .promote("future", "demo", None, &applied, &statuses, &delta)
            .await
            .err()
            .unwrap();
        assert!(error.to_string().contains("lower refused"));
        assert_eq!(
            failing.assurance.port().requests.lock().unwrap().len(),
            2,
            "third intent must not run after failure"
        );
        assert!(service(Some(1))
            .policy(
                detail(policy::Status::Stale { reasons: vec![] }).control,
                "demo",
                None,
                &detail(policy::Status::Stale { reasons: vec![] })
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("lower refused"));
    }
}
