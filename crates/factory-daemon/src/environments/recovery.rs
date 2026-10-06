//! Recovery is approved process work. It restarts/repairs what is installed;
//! it never writes a deployment or silently dispatches from a health tick.
use super::*;
use factory_core::config::SHELL_HARNESS;
use factory_core::environments::{Recover, RecoveryCommand, SamplePage, SampleQuery};
use factory_core::role::Grant;
use factory_core::task::NewTask;
use factory_core::workflow::{GateSpec, WorkflowDraft, WorkflowEdge, WorkflowNode, WorkflowNodeKind, WorkflowRun};
use factory_kernel::{RECOVERY_COMMIT_LABEL, RECOVERY_ENVIRONMENT_LABEL, RECOVERY_REASON_LABEL};

fn bad(message: impl Into<String>) -> FactoryError {
    FactoryError::BadRequest(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::role::Role;

    fn engine() -> (Arc<Engine>, PathBuf) {
        let (source, root) = super::super::tests::engine_with("  - name: prod\n    checks: [{ name: api, kind: command, command: 'true' }]\n    recover: { agent: operator, command: 'true' }\n");
        let mut factory = source.factory_snapshot();
        factory.config.scope = None;
        factory.config.scopes[0]
            .agents
            .push(serde_yaml_ng::from_str("name: operator\nharness: shell\nlifetime: task\nrole: foreman\n").unwrap());
        (
            Arc::new(
                Engine::new(
                    factory,
                    factory_plugins::Registry::with_builtins(),
                    source.l4.store.clone(),
                    PathBuf::from("factory"),
                    Vec::new(),
                )
                .with_environment_store(source.l1.environments.clone()),
            ),
            root,
        )
    }

    #[tokio::test]
    async fn recovery_authority_reason_and_healthy_checks_are_required_before_any_task() {
        let (engine, root) = engine();
        let agent =
            Caller::Agent { scope: "company".into(), name: "operator".into(), role: Role::foreman(), run_id: None };
        let request = |reason: &str| Recover { environment: "prod".into(), reason: reason.into() };
        assert!(matches!(engine.recover_environment(&agent, request("restart")).await, Err(FactoryError::Denied(_))));
        assert!(engine.recover_environment(&Caller::Owner, request(" \n ")).await.is_err());
        assert!(engine.recover_environment(&Caller::Owner, request(&"x".repeat(4001))).await.is_err());
        assert!(engine.l4.store.list(&factory_core::TaskFilter::default()).await.unwrap().is_empty());
        assert!(engine.check_environment("missing").await.is_err());
        assert!(engine.check_environment("prod").await.unwrap().ok);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn sample_query_rejects_unbounded_bad_scope_and_unknown_check_reads() {
        let (engine, root) = engine();
        let now = Utc::now();
        let query = SampleQuery {
            environment: "prod".into(),
            check: "api".into(),
            scope: Some("company".into()),
            from: Some(now - Duration::hours(1)),
            to: Some(now),
            before: None,
            limit: Some(5),
        };
        assert!(engine.environment_samples(query.clone()).await.unwrap().samples.is_empty());
        let mut factory = engine.factory_snapshot();
        let mut other: factory_core::config::Scope = serde_yaml_ng::from_str("id: other\nname: other\n").unwrap();
        other.path = PathBuf::from("projects/other");
        factory.config.scopes.push(other);
        let narrowed = Engine::new(
            factory,
            factory_plugins::Registry::with_builtins(),
            engine.l4.store.clone(),
            PathBuf::from("factory"),
            Vec::new(),
        )
        .with_environment_store(engine.l1.environments.clone());
        assert!(narrowed
            .environment_samples(SampleQuery { scope: Some("other".into()), ..query.clone() })
            .await
            .unwrap_err()
            .to_string()
            .contains("outside the selected scope"));
        for invalid in [
            SampleQuery { check: "missing".into(), ..query.clone() },
            SampleQuery { scope: Some("missing".into()), ..query.clone() },
            SampleQuery { limit: Some(501), ..query.clone() },
            SampleQuery { limit: Some(0), ..query.clone() },
            SampleQuery { before: Some(0), ..query.clone() },
            SampleQuery { from: Some(now - Duration::days(91)), ..query.clone() },
            SampleQuery { to: Some(now + Duration::hours(1)), ..query.clone() },
            SampleQuery { from: Some(now), to: Some(now), ..query.clone() },
        ] {
            assert!(engine.environment_samples(invalid).await.is_err());
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn recovery_fact_history_is_producer_owned_scoped_and_not_crowded_out_by_unrelated_work() {
        use crate::facts::{Facts, RecoveryQuery};
        let (engine, root) = engine();
        let run = engine
            .recover_environment(
                &Caller::Owner,
                Recover { environment: "prod".into(), reason: "repair installed system".into() },
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let facts = Facts::<factory_kernel::L6>::new(&engine)
                    .get::<factory_kernel::EnvironmentRecoveryFact>(&RecoveryQuery { scopes: None, limit: 1 })
                    .await
                    .unwrap();
                if facts[0].run.as_ref().is_some_and(|run| run.status == factory_core::RunStatus::Blocked) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("approval becomes a recorded blocked run");
        for _ in 0..10 {
            let definition = factory_core::workflow::WorkflowDefinition::from_draft(WorkflowDraft {
                name: "unrelated".into(),
                scope: "company".into(),
                ..Default::default()
            });
            engine.l4.workflows.put_run(&WorkflowRun::new(definition, Default::default())).await.unwrap();
        }
        let facts = Facts::<factory_kernel::L6>::new(&engine)
            .get::<factory_kernel::EnvironmentRecoveryFact>(&RecoveryQuery { scopes: None, limit: 1 })
            .await
            .unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].workflow_run_id, run.id);
        assert_eq!(facts[0].reason, "repair installed system");
        assert_eq!(facts[0].run.as_ref().unwrap().status, factory_core::RunStatus::Blocked);
        let foreign = Facts::<factory_kernel::L6>::new(&engine)
            .get::<factory_kernel::EnvironmentRecoveryFact>(&RecoveryQuery {
                scopes: Some(BTreeSet::from(["other".into()])),
                limit: 5,
            })
            .await
            .unwrap();
        assert!(foreign.is_empty());
        std::fs::remove_dir_all(root).ok();
    }
}
fn word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

impl Engine {
    pub(super) async fn validate_environment_agent(&self, scope: &str, name: &str) -> Result<()> {
        let (agent, harness, declaration) = self.resolve_agent(scope, name)?;
        if harness != SHELL_HARNESS || declaration.is_none() {
            return Err(bad("the environment recipe must name a declared shell agent"));
        }
        let role = self.effective_role(scope, &agent).await;
        if self
            .roles_for(scope)
            .get(&role)
            .is_none_or(|role| !role.allows(Grant::DeployRecord) || !role.allows(Grant::TaskReport))
        {
            return Err(bad("the environment agent needs task.report and deploy.record in its scope"));
        }
        Ok(())
    }

    pub(super) async fn recovery_recipe(&self, scope: &str, environment: &EnvironmentDecl) -> Result<RecoveryCommand> {
        if environment.paused || environment.checks.is_empty() {
            return Err(bad("recovery needs declared, unpaused health checks"));
        }
        let recipe = environment.recover.clone().ok_or_else(|| bad("no recovery recipe is declared"))?;
        self.validate_environment_agent(scope, &recipe.agent).await?;
        Ok(recipe)
    }

    pub(crate) async fn recover_environment(
        self: &Arc<Self>,
        caller: &Caller,
        request: Recover,
    ) -> Result<WorkflowRun> {
        if !matches!(caller, Caller::Owner) {
            return Err(FactoryError::Denied("starting a recovery is the owner's action".into()));
        }
        let reason = request.reason.trim();
        if reason.is_empty() || reason.len() > 4000 {
            return Err(bad("recovery needs a nonempty reason of at most 4000 bytes"));
        }
        let _guard = self.l1.promotion_lock.lock().await;
        let (scope, environment) = self
            .factory_snapshot()
            .config
            .environments()
            .into_iter()
            .find(|(_, declaration)| declaration.name == request.environment)
            .ok_or_else(|| bad("the environment is not declared"))?;
        let recipe = self.recovery_recipe(&scope, &environment).await?;
        let timeout = recipe.timeout_seconds();
        if self.l4.workflows.active_runs().await?.iter().any(|run| operation_targets(run, &environment.name)) {
            return Err(bad("an environment operation is already pending; finish or cancel it first"));
        }
        let deployments = self.l1.environments.deployments().await?;
        if deployments
            .iter()
            .any(|deployment| deployment.environment == environment.name && deployment.status == DeployStatus::Running)
        {
            return Err(bad("a deployment is already running on the environment"));
        }
        let expected = deployments
            .iter()
            .find(|deployment| {
                deployment.environment == environment.name && deployment.status == DeployStatus::Succeeded
            })
            .map(|deployment| deployment.release.commit.clone());
        let mut labels = BTreeMap::from([
            (RECOVERY_ENVIRONMENT_LABEL.into(), environment.name.clone()),
            (RECOVERY_REASON_LABEL.into(), reason.to_owned()),
        ]);
        if let Some(commit) = &expected {
            labels.insert(RECOVERY_COMMIT_LABEL.into(), commit.clone());
        }
        let instructions = format!(
            "set -eu\nexport FACTORY_ENVIRONMENT={}\nexport FACTORY_RECOVERY_REASON={}\nexport FACTORY_EXPECTED_RELEASE_COMMIT={}\nsh -c {}\n{} environment-check {}\n",
            word(&environment.name), word(reason), word(expected.as_deref().unwrap_or("")), word(&recipe.command), word(&self.shared.factory_bin.to_string_lossy()), word(&environment.name),
        );
        let definition = self
            .create_workflow(WorkflowDraft {
                name: format!("Recover {}", environment.name),
                description: format!(
                    "{reason}\nRestart or repair the installed system; never build or deploy a new release."
                ),
                scope: scope.clone(),
                category: Some("recovery".into()),
                nodes: vec![
                    WorkflowNode {
                        id: "approve-recovery".into(),
                        session: Default::default(),
                        position: Default::default(),
                        kind: WorkflowNodeKind::Approval,
                        task: NewTask {
                            title: format!("Approve recovery of {}", environment.name),
                            ..Default::default()
                        },
                        gate: Some(GateSpec {
                            step: "environment-recovery".into(),
                            subject: Some("recover".into()),
                            by: Some("person".into()),
                            actor: Some("owner".into()),
                            locked: true,
                            ..Default::default()
                        }),
                        exits: vec![],
                        expand: None,
                    },
                    WorkflowNode {
                        id: "recover".into(),
                        session: Default::default(),
                        position: Default::default(),
                        kind: WorkflowNodeKind::Task,
                        task: NewTask {
                            title: format!("Recover {}", environment.name),
                            instructions,
                            scope: Some(scope),
                            agent: Some(recipe.agent),
                            category: Some("recovery".into()),
                            worktree: Some(false),
                            timeout_seconds: Some(timeout),
                            labels,
                            ..Default::default()
                        },
                        gate: None,
                        exits: vec![],
                        expand: None,
                    },
                ],
                edges: vec![WorkflowEdge {
                    id: "approval-recover".into(),
                    from: "approve-recovery".into(),
                    to: "recover".into(),
                }],
                ..Default::default()
            })
            .await?;
        self.start_workflow(&definition.id, Default::default(), caller).await
    }

    /// A run may verify the repaired environment using deploy.record's
    /// existing scope/reach checks. Missing or paused checks are not success.
    pub(crate) async fn check_environment(&self, environment: &str) -> Result<DeployVerification> {
        self.verify_environment(environment)
            .await
            .ok_or_else(|| bad("verification needs declared, unpaused health checks"))
    }

    pub(crate) async fn environment_samples(&self, query: SampleQuery) -> Result<SamplePage> {
        let snapshot = self.factory_snapshot();
        let (scope, declaration) = snapshot
            .config
            .environments()
            .into_iter()
            .find(|(_, declaration)| declaration.name == query.environment)
            .ok_or_else(|| bad("the environment is not declared"))?;
        if !declaration.checks.iter().any(|check| check.display_name() == query.check) {
            return Err(bad("the check is not declared on this environment"));
        }
        if let Some(asked) = &query.scope {
            let (root, children) = factory_core::config::subtree_scopes(&snapshot, Some(asked))?;
            if !root.iter().chain(&children).any(|member| member.name == scope) {
                return Err(bad("the environment is outside the selected scope"));
            }
        }
        let now = Utc::now();
        let to = query.to.unwrap_or(now);
        let from = query.from.unwrap_or(to - Duration::hours(24));
        let limit = query.limit.unwrap_or(200);
        if from >= to || from < now - Duration::days(env::SAMPLE_RETENTION_DAYS) || to > now + Duration::seconds(5) {
            return Err(bad("sample window must be nonempty, within the retained 90 days and not in the future"));
        }
        if !(1..=500).contains(&limit) || query.before.is_some_and(|cursor| cursor <= 0) {
            return Err(bad("sample limit is 1..500 and the before cursor must be positive"));
        }
        self.l1.environments.sample_page(query.environment, query.check, from, to, query.before, limit).await
    }
}
