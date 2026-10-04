//! L5 owns remediation submission and Quality's refusal/deduplication rules.
use crate::{
    checks::Check,
    quality::{self, Measure, ScenarioStatus, ScopeReport},
};
use factory_kernel::{
    CommandPort, Commands, FactoryError, Facts, Provide, Result, TaskInventoryFact,
    TaskInventoryQuery, TaskReceipt, L5,
};
use factory_process::{creation::TaskCommands, task::NewTask};
use std::collections::BTreeMap;

/// L6 supplies intent, never an L4 task or task-state request.
pub struct Intent {
    pub title: String,
    pub instructions: String,
    pub scope: String,
    pub agent: Option<String>,
    pub labels: BTreeMap<String, String>,
}
#[async_trait::async_trait]
pub trait RemediationCommands: CommandPort<Level = L5> + Send + Sync {
    async fn remediate(&self, intent: Intent) -> Result<TaskReceipt>;
}
#[async_trait::async_trait]
impl<P: RemediationCommands + ?Sized> RemediationCommands for &P {
    async fn remediate(&self, intent: Intent) -> Result<TaskReceipt> {
        (**self).remediate(intent).await
    }
}
#[derive(Debug)]
pub struct QualityReceipt {
    pub task: TaskReceipt,
    pub created: bool,
}
pub struct Service<P: TaskCommands, I> {
    pub tasks: Commands<L5, P>,
    pub inventory: I,
}
impl<P: TaskCommands, I> CommandPort for Service<P, I> {
    type Level = L5;
}
#[async_trait::async_trait]
impl<P: TaskCommands, I: Send + Sync> RemediationCommands for Service<P, I> {
    async fn remediate(&self, intent: Intent) -> Result<TaskReceipt> {
        self.tasks
            .port()
            .submit(NewTask {
                title: intent.title,
                instructions: intent.instructions,
                scope: Some(intent.scope),
                agent: intent.agent,
                labels: intent.labels,
                ..Default::default()
            })
            .await
    }
}
impl<
        P: TaskCommands,
        I: Provide<
            TaskInventoryFact,
            Query = TaskInventoryQuery,
            Value = Vec<TaskInventoryFact>,
            Error = FactoryError,
        >,
    > Service<P, I>
{
    async fn inventory(&self, scope: &str) -> Result<Vec<TaskInventoryFact>> {
        Facts::<L5>::new()
            .get::<TaskInventoryFact, _>(
                &self.inventory,
                &TaskInventoryQuery::Exact(scope.to_string()),
            )
            .await
    }
    pub async fn quality(
        &self,
        scope: &str,
        attribute: &str,
        scenario: &str,
        agent: Option<String>,
        report: &ScopeReport,
    ) -> Result<QualityReceipt> {
        let result = report
            .attributes
            .iter()
            .find(|a| a.id == attribute)
            .and_then(|a| {
                a.scenarios
                    .iter()
                    .find(|s| s.scenario.scenario.id == scenario)
            })
            .ok_or_else(|| {
                FactoryError::BadRequest(format!(
                    "{attribute}/{scenario} is not a quality scenario that applies at {scope:?}"
                ))
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

        let label = remediation_label(scope, attribute, scenario);
        let existing = self.inventory(scope).await?.into_iter().find(|t| {
            t.open && t.labels.get("quality").map(String::as_str) == Some(label.as_str())
        });
        if let Some(task) = existing {
            return Ok(QualityReceipt {
                task: TaskReceipt { id: task.id },
                created: false,
            });
        }

        let mut labels = BTreeMap::new();
        labels.insert("quality".to_string(), label);
        let new_task = NewTask {
            title: format!("Meet quality scenario {attribute}/{scenario}"),
            instructions: remediation_instructions(scope, attribute, result),
            scope: Some(scope.to_string()),
            agent,
            labels,
            ..Default::default()
        };
        Ok(QualityReceipt {
            task: self.tasks.port().submit(new_task).await?,
            created: true,
        })
    }
}
pub fn remediation_label(scope: &str, attribute: &str, scenario: &str) -> String {
    format!("{scope}/{attribute}/{scenario}")
}

pub fn remediation_instructions(
    scope: &str,
    attribute: &str,
    result: &quality::ScenarioResult,
) -> String {
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
        out.push_str(&format!(
            "\nResponse measure: {}\n",
            quality::describe_measure(measure)
        ));
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

pub fn unfixable_by_a_task(measure: Option<&Measure>) -> Option<String> {
    match measure? {
        Measure::Check(Check::Attestation) => Some(quality::ATTESTATION_UNSUPPORTED.to_string()),
        Measure::Check(_) => None,
        Measure::Metric(m) if quality::is_quality_metric(&m.metric) => Some(format!(
            "{} is computed from quality scenarios themselves, so it cannot measure one",
            m.metric
        )),
        Measure::Metric(m) => match crate::metrics::resolve(&m.metric) {
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    struct Tasks {
        requests: Mutex<Vec<NewTask>>,
        fail: bool,
    }
    impl CommandPort for Tasks {
        type Level = factory_kernel::L4;
    }
    #[async_trait::async_trait]
    impl TaskCommands for Tasks {
        async fn submit(&self, new: NewTask) -> Result<TaskReceipt> {
            self.requests.lock().unwrap().push(new);
            if self.fail {
                return Err(FactoryError::BadRequest("submission refused".into()));
            }
            Ok(TaskReceipt {
                id: "created".into(),
            })
        }
    }
    struct Inventory {
        rows: Mutex<Vec<TaskInventoryFact>>,
        reads: Mutex<Vec<String>>,
        fail: bool,
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
                panic!("remediation must use exact scope")
            };
            self.reads.lock().unwrap().push(scope.clone());
            if self.fail {
                return Err(FactoryError::BadRequest("inventory refused".into()));
            }
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
    fn service(fail: bool) -> Service<Tasks, Inventory> {
        Service {
            tasks: Commands::new(Tasks {
                requests: Mutex::new(vec![]),
                fail,
            }),
            inventory: Inventory {
                rows: Mutex::new(vec![]),
                reads: Mutex::new(vec![]),
                fail: false,
            },
        }
    }
    fn report(status: ScenarioStatus) -> ScopeReport {
        serde_json::from_value(serde_json::json!({
            "scope":"demo","profiles":["base"],"tradeoffs":[],
            "attributes":[{
                "id":"reliability","characteristic":"reliability","importance":"H","difficulty":"L",
                "declared_at":{"scope":"demo","profile":"base"},"status":status,
                "scenarios":[{
                    "id":"restore","response":"Restore the file",
                    "measure":{"check":"task","task":"restore"},
                    "declared_at":{"scope":"demo","profile":"base"},
                    "status":status,"reasons":["No completed task"]
                }]
            }]
        }))
        .unwrap()
    }
    #[tokio::test]
    async fn intent_submits_exact_payload_and_returns_only_acknowledgement() {
        let s = service(false);
        let labels = BTreeMap::from([
            ("policy".into(), "house/restore".into()),
            ("scenario".into(), "future".into()),
        ]);
        let receipt = s
            .remediate(Intent {
                title: "Prepare".into(),
                instructions: "Exact instructions".into(),
                scope: "demo".into(),
                agent: Some("worker".into()),
                labels: labels.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            receipt,
            TaskReceipt {
                id: "created".into()
            }
        );
        let requests = s.tasks.port().requests.lock().unwrap();
        let task = &requests[0];
        assert_eq!(
            (&task.title, &task.instructions),
            (&"Prepare".to_string(), &"Exact instructions".to_string())
        );
        assert_eq!(
            (&task.scope, &task.agent),
            (&Some("demo".into()), &Some("worker".into()))
        );
        assert_eq!(task.labels, labels);
        assert!(task.schedule.is_none() && task.after.is_none() && task.runtime.is_none());
        assert!(
            s.inventory.reads.lock().unwrap().is_empty(),
            "intent submission is not another evaluation"
        );
    }
    #[tokio::test]
    async fn quality_reads_live_exact_inventory_keeps_order_and_only_submits_after_closed() {
        let s = service(false);
        let row = |id: &str, scope: &str, open: bool| TaskInventoryFact {
            id: id.into(),
            title: id.into(),
            scope: scope.into(),
            open,
            labels: BTreeMap::from([("quality".into(), "demo/reliability/restore".into())]),
        };
        *s.inventory.rows.lock().unwrap() = vec![
            row("other", "child", true),
            row("closed", "demo", false),
            row("newest", "demo", true),
            row("older", "demo", true),
        ];
        let r = report(ScenarioStatus::NotMet);
        let first = s
            .quality("demo", "reliability", "restore", None, &r)
            .await
            .unwrap();
        assert!(!first.created);
        assert_eq!(first.task.id, "newest");
        assert!(s.tasks.port().requests.lock().unwrap().is_empty());
        for row in s.inventory.rows.lock().unwrap().iter_mut() {
            row.open = false;
        }
        let second = s
            .quality("demo", "reliability", "restore", Some("worker".into()), &r)
            .await
            .unwrap();
        assert!(second.created);
        assert_eq!(second.task.id, "created");
        let requests = s.tasks.port().requests.lock().unwrap();
        assert_eq!(
            requests[0].title,
            "Meet quality scenario reliability/restore"
        );
        assert_eq!(
            requests[0].instructions,
            remediation_instructions("demo", "reliability", &r.attributes[0].scenarios[0])
        );
        assert_eq!(requests[0].labels["quality"], "demo/reliability/restore");
        assert_eq!(*s.inventory.reads.lock().unwrap(), vec!["demo", "demo"]);
    }
    #[tokio::test]
    async fn quality_refusals_precede_inventory_and_lower_errors_are_not_success() {
        let mut s = service(false);
        for status in [ScenarioStatus::Met, ScenarioStatus::Draft] {
            assert!(s
                .quality("demo", "reliability", "restore", None, &report(status))
                .await
                .is_err());
        }
        let mut no_data = report(ScenarioStatus::NoData);
        no_data.attributes[0].scenarios[0].scenario.scenario.measure =
            Some(Measure::Check(Check::Attestation));
        assert!(s
            .quality("demo", "reliability", "restore", None, &no_data)
            .await
            .unwrap_err()
            .to_string()
            .contains("no task can change"));
        assert!(s.inventory.reads.lock().unwrap().is_empty());
        assert!(s.tasks.port().requests.lock().unwrap().is_empty());
        s.inventory.fail = true;
        assert!(s
            .quality(
                "demo",
                "reliability",
                "restore",
                None,
                &report(ScenarioStatus::NotMet)
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("inventory refused"));
        assert!(s.tasks.port().requests.lock().unwrap().is_empty());
        let failing = service(true);
        assert!(failing
            .quality(
                "demo",
                "reliability",
                "restore",
                None,
                &report(ScenarioStatus::Stale)
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("submission refused"));
    }
}
