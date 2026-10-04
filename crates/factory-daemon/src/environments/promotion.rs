//! Owner-requested promotions are ordinary release workflows, not deployment
//! commands run by the web handler. Policy gates judge a preflight workspace
//! before a separate, mandatory owner approval can release the deploy task.
use super::*;
use factory_core::environments::Promote;
use factory_core::task::NewTask;
use factory_core::workflow::{GateSpec, WorkflowDraft, WorkflowEdge, WorkflowNode, WorkflowNodeKind, WorkflowRun};

const TARGET_LABEL: &str = "factory.promotion.target";

fn bad(message: impl Into<String>) -> FactoryError {
    FactoryError::BadRequest(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(role: &str, cross_scope: bool) -> (Arc<Engine>, PathBuf) {
        let (source, root) = super::super::tests::engine_with(
            "  - name: staging\n    promotes_to: production\n    checks: [{ kind: command, command: 'true', name: api }]\n  - name: production\n    checks: [{ kind: command, command: 'true', name: api }]\n    deploy: { agent: releaser, command: 'true' }\n",
        );
        let mut factory = source.factory_snapshot();
        // This fixture listed its root twice. Keep the discovery-style list
        // authoritative while moving declarations to another scope.
        factory.config.scope = None;
        let agent = serde_yaml_ng::from_str(&format!("name: releaser\nharness: shell\nlifetime: task\nrole: {role}\n"))
            .unwrap();
        if cross_scope {
            let mut target: factory_core::config::Scope =
                serde_yaml_ng::from_str("id: target-id\nname: target\n").unwrap();
            target.path = PathBuf::from("projects/target");
            target.agents.push(agent);
            target.environments.push(factory.config.scopes[0].environments.remove(1));
            factory.config.scopes.push(target);
        } else {
            factory.config.scopes[0].agents.push(agent);
        }
        let engine = Arc::new(
            Engine::new(
                factory,
                factory_plugins::Registry::with_builtins(),
                source.store.clone(),
                PathBuf::from("factory"),
                Vec::new(),
            )
            .with_environment_store(source.environments.clone()),
        );
        (engine, root)
    }

    async fn selected(engine: &Engine, verify: bool) -> Deployment {
        let deployment = engine
            .deploy_start(
                &Caller::Owner,
                env::DeployStart {
                    environment: "staging".into(),
                    scope: None,
                    strict_verification: false,
                    release: env::ReleaseFacts { commit: "a".repeat(40), ..Default::default() },
                    via: None,
                    started_at: None,
                },
            )
            .await
            .unwrap();
        engine
            .deploy_finish(DeployFinish { id: deployment.id, status: DeployStatus::Succeeded, verify, reason: None })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn readiness_uses_a_verified_selection_and_an_authorized_declared_release_agent() {
        let (engine, root) = fixture("foreman", false);
        let source = &engine.factory_snapshot().config.environments()[0].1;
        let mut current = selected(&engine, false).await;
        assert!(engine
            .promotion_target(source, Some(&current), &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("not verified"));
        current = selected(&engine, true).await;
        assert!(engine.promotion_target(source, Some(&current), &[]).await.is_ok());
        current.release.dirty = true;
        assert!(engine
            .promotion_target(source, Some(&current), &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("clean release"));
        current.release.dirty = false;
        current.release.commit = "main".into();
        assert!(engine.promotion_target(source, Some(&current), &[]).await.is_err());
        std::fs::remove_dir_all(root).ok();
        let (engine, root) = fixture("worker", false);
        let current = selected(&engine, true).await;
        let source = &engine.factory_snapshot().config.environments()[0].1;
        assert!(engine
            .promotion_target(source, Some(&current), &[])
            .await
            .unwrap_err()
            .to_string()
            .contains("deploy.record"));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn scoped_readiness_can_target_another_scope_without_leaking_its_history() {
        let (engine, root) = fixture("foreman", true);
        selected(&engine, true).await;
        let report = engine.environments_report(Some("company".into())).await.unwrap();
        // target is a descendant, so filter a source-only scope in a sibling
        // position rather than relying on display names as ancestry.
        let mut factory = engine.factory_snapshot();
        factory.config.scopes[0].path = PathBuf::from("projects/source");
        let narrowed = Engine::new(
            factory,
            factory_plugins::Registry::with_builtins(),
            engine.store.clone(),
            PathBuf::from("factory"),
            Vec::new(),
        )
        .with_environment_store(engine.environments.clone());
        let source = narrowed.environments_report(Some("company".into())).await.unwrap();
        assert_eq!(source.environments.len(), 1);
        assert!(source.environments[0].promotion_ready, "{:?}", source.environments[0].promotion_reason);
        assert_eq!(source.environments[0].promotes_to.as_deref(), Some("production"));
        assert_eq!(report.environments.len(), 2);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn stale_or_missing_repository_selection_creates_no_release_tasks() {
        let (engine, root) = fixture("foreman", false);
        let current = selected(&engine, true).await;
        let error = engine
            .promote_environment(&Caller::Owner, Promote { environment: "staging".into(), deployment: "old".into() })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("source deployment changed"));
        let error = engine
            .promote_environment(&Caller::Owner, Promote { environment: "staging".into(), deployment: current.id })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("not a commit"), "{error}");
        assert!(engine.store.list(&factory_core::TaskFilter::default()).await.unwrap().is_empty());
        assert!(engine.workflows.active_runs().await.unwrap().is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn shell_words_preserve_quotes_spaces_and_metacharacters_without_expansion() {
        let value = "release 'one'; $(touch should-not-exist)\nwith spaces";
        let output =
            std::process::Command::new("sh").args(["-c", &format!("printf '%s' {}", word(value))]).output().unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), value);
    }
}

fn word(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\"'\"'"))
}

impl Engine {
    async fn promotion_recipe(&self, scope: &str, environment: &EnvironmentDecl) -> Result<env::ReleaseCommand> {
        let recipe = environment.deploy.clone().ok_or_else(|| bad("the target has no deploy recipe"))?;
        self.validate_environment_agent(scope, &recipe.agent).await?;
        Ok(recipe)
    }

    /// Readiness uses the whole instance, even when the page is narrowed to
    /// a source scope whose promotion target is in another scope.
    pub(super) async fn promotion_target(
        &self,
        source: &EnvironmentDecl,
        current: Option<&Deployment>,
        history: &[Deployment],
    ) -> Result<(String, EnvironmentDecl, env::ReleaseCommand)> {
        if source.paused {
            return Err(bad("source checks are paused"));
        }
        let next = source.promotes_to.as_ref().ok_or_else(|| bad("no promotion target is declared"))?;
        let (scope, target) = self
            .factory_snapshot()
            .config
            .environments()
            .into_iter()
            .find(|(_, environment)| environment.name == *next)
            .ok_or_else(|| bad("the promotion target is no longer declared"))?;
        if target.paused || target.checks.is_empty() {
            return Err(bad("the target needs declared, unpaused health checks"));
        }
        let current = current.ok_or_else(|| bad("the source has no successful deployment"))?;
        if current.release.dirty
            || !matches!(current.release.commit.len(), 40 | 64)
            || !current.release.commit.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(bad("promotion needs a clean release recorded by its full commit id, not a moving ref"));
        }
        if current.verification.as_ref().is_none_or(|verification| !verification.ok || verification.checks.is_empty()) {
            return Err(bad("the selected source deployment was not verified"));
        }
        if history.iter().any(|deployment| {
            deployment.status == DeployStatus::Running
                && (deployment.environment == source.name || deployment.environment == target.name)
        }) {
            return Err(bad("a deployment is already running on the source or target"));
        }
        if history
            .iter()
            .find(|deployment| deployment.environment == target.name && deployment.status == DeployStatus::Succeeded)
            .is_some_and(|deployment| deployment.release.commit == current.release.commit)
        {
            return Err(bad("the target already runs this release"));
        }
        let recipe = self.promotion_recipe(&scope, &target).await?;
        Ok((scope, target, recipe))
    }

    pub(crate) async fn promote_environment(
        self: &Arc<Self>,
        caller: &Caller,
        request: Promote,
    ) -> Result<WorkflowRun> {
        if !matches!(caller, Caller::Owner) {
            return Err(FactoryError::Denied("starting a promotion is the owner's action".into()));
        }
        let _guard = self.promotion_lock.lock().await;
        let source = self
            .factory_snapshot()
            .config
            .environments()
            .into_iter()
            .find(|(_, environment)| environment.name == request.environment)
            .map(|(_, environment)| environment)
            .ok_or_else(|| bad("the source environment is not declared"))?;
        let history = self.environments.deployments().await?;
        let current = history
            .iter()
            .find(|deployment| deployment.environment == source.name && deployment.status == DeployStatus::Succeeded);
        if current.is_none_or(|deployment| deployment.id != request.deployment) {
            return Err(bad("the source deployment changed; refresh and select its current release"));
        }
        let current = current.expect("checked current deployment");
        let (scope, target, recipe) = self.promotion_target(&source, Some(current), &history).await?;
        let pending = self.workflows.active_runs().await?.into_iter().any(|run| operation_targets(&run, &target.name));
        if pending {
            return Err(bad("a promotion to this target is already pending; finish or cancel it first"));
        }

        let commit = &current.release.commit;
        let environment = format!(
            "set -eu\nexport FACTORY_RELEASE_COMMIT={}\nexport FACTORY_ENVIRONMENT={}\nexport FACTORY_SOURCE_ENVIRONMENT={}\n\
             test \"$(git rev-parse HEAD)\" = \"$FACTORY_RELEASE_COMMIT\"\n\
             git diff --quiet HEAD --\n",
            word(commit), word(&target.name), word(&source.name),
        );
        let binary = word(&self.factory_bin.to_string_lossy());
        let mut args = format!(
            "--env {} --scope {} --commit {} --strict-verification --via promotion",
            word(&target.name),
            word(&scope),
            word(commit)
        );
        for (flag, value) in [
            ("--describe", &current.release.describe),
            ("--version", &current.release.version),
            ("--profile", &current.release.profile),
            ("--source", &current.release.source),
            ("--build-run", &current.release.build_run),
            ("--build-scope", &current.release.build_scope),
        ] {
            if let Some(value) = value {
                args.push_str(&format!(" {flag} {}", word(value)));
            }
        }
        if let Some(at) = current.release.committed_at {
            args.push_str(&format!(" --committed-at {}", word(&at.to_rfc3339())));
        }
        let on_exit = word(&format!("if [ \"$promotion_finished\" -eq 0 ]; then {binary} deploy finish \"$promotion_deploy_id\" --status failed --reason \"promotion command ended without verified success\" || true; fi"));
        let instructions = format!(
            "{environment}promotion_deploy_id=\"$({binary} deploy start {args})\"\n\
             promotion_finished=0\n\
             trap {on_exit} 0\n\
             sh -c {}\n\
             test \"$(git rev-parse HEAD)\" = \"$FACTORY_RELEASE_COMMIT\"\n\
             git diff --quiet HEAD --\n\
             {binary} deploy finish \"$promotion_deploy_id\" --status succeeded\n\
             promotion_finished=1\n",
            word(&recipe.command),
        );
        let task = |id: &str, title: String, instructions: String| WorkflowNode {
            id: id.into(),
            session: Default::default(),
            position: Default::default(),
            kind: WorkflowNodeKind::Task,
            task: NewTask {
                title,
                instructions,
                scope: Some(scope.clone()),
                agent: Some(recipe.agent.clone()),
                category: Some("release".into()),
                worktree: Some(true),
                timeout_seconds: Some(recipe.timeout_seconds()),
                labels: BTreeMap::from([(TARGET_LABEL.into(), target.name.clone())]),
                ..Default::default()
            },
            gate: None,
            exits: Vec::new(),
            expand: None,
        };
        let mut draft = WorkflowDraft {
            name: format!("Promote {} to {}: {}", source.name, target.name, &commit[..10]),
            description: format!("Selected verified deployment {} of {}. Commit and recipe are frozen; owner approval is still required.", current.id, source.name),
            scope: scope.clone(), workspace_ref: Some(commit.clone()), category: Some("release".into()),
            nodes: vec![
                task("preflight", "Release preflight".into(), format!("{environment}sh -c {}", word(recipe.prepare.as_deref().unwrap_or("true")))),
                WorkflowNode { id: "approve-deployment".into(), session: Default::default(), position: Default::default(),
                    kind: WorkflowNodeKind::Approval, task: NewTask { title: format!("Approve {} to {}", &commit[..10], target.name), ..Default::default() },
                    gate: Some(GateSpec { step: "environment-promotion".into(), subject: Some("deploy".into()),
                        by: Some("person".into()), actor: Some("owner".into()), locked: true, ..Default::default() }),
                    exits: Vec::new(), expand: None },
                task("deploy", format!("Deploy {} to {}", &commit[..10], target.name), instructions),
            ],
            edges: vec![WorkflowEdge { id: "preflight-approval".into(), from: "preflight".into(), to: "approve-deployment".into() },
                WorkflowEdge { id: "approval-deploy".into(), from: "approve-deployment".into(), to: "deploy".into() }],
            ..Default::default()
        };
        // Validate target repository membership before leaving any definition
        // behind. start_workflow freezes it again before persisting the run.
        let mut definition = factory_core::workflow::WorkflowDefinition::from_draft(draft.clone());
        definition.validate().map_err(bad)?;
        self.freeze_workflow_workspace(&mut definition).await?;
        if definition.workspace_ref.as_deref() != Some(commit) {
            return Err(bad("target repository resolved another commit"));
        }
        draft.workspace_ref = definition.workspace_ref;
        let definition = self.create_workflow(draft).await?;
        self.start_workflow(&definition.id, Default::default(), caller).await
    }

    /// A task ending without a finish receipt is a failed deployment, never
    /// inferred success. Also used during startup recovery.
    pub(crate) async fn settle_run_deployments(&self, run: &factory_core::run::Run) {
        if !run.status.is_terminal() {
            return;
        }
        let Ok(history) = self.environments.deployments().await else {
            return;
        };
        for deployment in history.into_iter().filter(|deployment| {
            deployment.status == DeployStatus::Running && deployment.actor.run_id.as_deref() == Some(&run.id)
        }) {
            let request = DeployFinish {
                id: deployment.id,
                status: DeployStatus::Failed,
                verify: false,
                reason: Some(format!(
                    "release run {} ended as {} without a deploy finish receipt",
                    run.id,
                    run.status.as_str()
                )),
            };
            if let Err(error) = self.deploy_finish(request).await {
                tracing::warn!(run = run.id, "could not settle unfinished deployment: {error}");
            }
        }
    }

    pub(crate) async fn reconcile_run_deployments(&self) {
        let Ok(history) = self.environments.deployments().await else {
            return;
        };
        let runs: BTreeSet<String> = history
            .into_iter()
            .filter(|deployment| deployment.status == DeployStatus::Running)
            .filter_map(|deployment| deployment.actor.run_id)
            .collect();
        for id in runs {
            if let Ok(Some(run)) = self.store.get_run(&id).await {
                self.settle_run_deployments(&run).await;
            }
        }
    }
}
