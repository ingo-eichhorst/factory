//! The live L5 workflow preview owner. Authored L6 requirements enter as
//! raw plan inputs; L4 blueprints and L3 declaration names are typed facts.
//! No Engine callback, upper report or precompiled plan enters this service.
use crate::plan_service;
use factory_kernel::{
    CompiledPlanFact, ControlPlan, FactProvider, FactoryError, Facts, FunctionaryRosterFact,
    Provide, Result, StepKind, WorkflowBlueprintFact, WorkflowBlueprintQuery, WorkflowEnforcement,
    WorkflowEnforcementFinding, WorkflowPreviewFact, WorkflowTargetsFact, L5,
};
use factory_process::workflow::{PartShape, WorkflowDefinition, WorkflowLint, WorkflowNodeKind};
use std::collections::BTreeMap;

pub struct Subject {
    pub workflow: Option<String>,
    pub task: Option<String>,
    pub scope: Option<String>,
    pub category: Option<String>,
}

/// Private L5 preparation, not a cross-level report or fact payload.
pub struct Preparation {
    definition: Option<WorkflowDefinition>,
    part: Option<PartShape>,
    subject: String,
    scope: String,
    categories: Vec<String>,
}
impl Preparation {
    pub fn scope(&self) -> &str {
        &self.scope
    }
    pub fn categories(&self) -> &[String] {
        &self.categories
    }
}

pub struct Read {
    pub blueprint: WorkflowBlueprintFact,
    /// An authored read failure is deferred until after blueprint validation,
    /// just as in the original request path. It is not a computed plan.
    pub requirements: Vec<Result<plan_service::Read>>,
}

pub struct Provider<R> {
    plans: plan_service::Provider,
    roster: R,
    default_agent: String,
}
impl<R> Provider<R> {
    pub fn new(plans: plan_service::Provider, roster: R, default_agent: String) -> Self {
        Self {
            plans,
            roster,
            default_agent,
        }
    }

    fn prepare_definition(definition: WorkflowDefinition) -> Result<Preparation> {
        definition.validate().map_err(FactoryError::BadRequest)?;
        let part = match &definition.part {
            Some(_) => Some(definition.part_shape().map_err(FactoryError::BadRequest)?),
            None => None,
        };
        let subject = definition.id.clone();
        let scope = definition.scope.clone();
        let definition = match &part {
            Some(shape) => definition.part_preview(shape),
            None => definition,
        };
        let categories = definition.categories().into_iter().collect();
        Ok(Preparation {
            definition: Some(definition),
            part,
            subject,
            scope,
            categories,
        })
    }

    pub async fn prepare<B>(&self, subject: Subject, blueprints: &B) -> Result<Preparation>
    where
        B: Provide<
            WorkflowBlueprintFact,
            Query = WorkflowBlueprintQuery,
            Value = Vec<WorkflowBlueprintFact>,
            Error = FactoryError,
        >,
    {
        let (query, task) = match (subject.workflow, subject.task) {
            (Some(_), Some(_)) => {
                return Err(FactoryError::BadRequest(
                    "lint a workflow or a task, not both".into(),
                ))
            }
            (Some(id), None) => (Some(WorkflowBlueprintQuery::Workflow(id)), false),
            (None, Some(id)) => (Some(WorkflowBlueprintQuery::Task(id)), true),
            (None, None) => (None, false),
        };
        let Some(query) = query else {
            let scope = subject.scope.ok_or_else(|| {
                FactoryError::BadRequest("name a workflow, a task, or a scope to lint".into())
            })?;
            return Ok(Preparation {
                definition: None,
                part: None,
                subject: String::new(),
                scope,
                categories: vec![
                    factory_kernel::effective_category(subject.category.as_deref()).into(),
                ],
            });
        };
        let mut facts = Facts::<L5>::new()
            .get::<WorkflowBlueprintFact, _>(blueprints, &query)
            .await?;
        let fact = facts.pop().ok_or_else(|| {
            FactoryError::Other(anyhow::anyhow!(
                "workflow blueprint provider returned no subject"
            ))
        })?;
        let mut definition = factory_process::workflow_blueprints::definition(&fact)?;
        if let Some(category) = subject.category {
            definition.category = Some(category.clone());
            if task {
                // The original override was applied before implicit(task).
                for node in &mut definition.nodes {
                    node.task.category = Some(category.clone());
                }
            }
        }
        Self::prepare_definition(definition)
    }
}
impl<R> Provider<R>
where
    R: Provide<
        FunctionaryRosterFact,
        Query = String,
        Value = FunctionaryRosterFact,
        Error = FactoryError,
    >,
{
    /// Live same-level Quality compilation, followed by L3 declaration facts.
    /// Callers provide authored sources only, never resolved plan results.
    pub async fn finish(
        &self,
        prepared: Preparation,
        requirements: Vec<Result<plan_service::Read>>,
    ) -> Result<WorkflowLint> {
        let mut plans = BTreeMap::<String, ControlPlan>::new();
        for read in requirements {
            let read = read?;
            let plan =
                <plan_service::Provider as Provide<CompiledPlanFact>>::get(&self.plans, &read)
                    .await?
                    .plan;
            plans.insert(read.category, plan);
        }
        if plans.keys().cloned().collect::<Vec<_>>() != prepared.categories {
            return Err(FactoryError::Other(anyhow::anyhow!(
                "workflow plan categories do not match their subject"
            )));
        }
        let Some(definition) = prepared.definition else {
            let plan = plans
                .into_values()
                .next()
                .ok_or_else(|| FactoryError::Other(anyhow::anyhow!("missing scope plan")))?;
            return Ok(WorkflowLint {
                subject: String::new(),
                scope: plan.scope.clone(),
                plans: vec![plan],
                injections: Vec::new(),
                violations: Vec::new(),
                injected: None,
                part: None,
            });
        };
        let (mut injected, injections) = definition.inject(&plans);
        self.bind_functionaries(&mut injected).await?;
        let mut violations = injected.ordering_violations(&plans);
        for node in injected
            .nodes
            .iter()
            .filter(|n| n.kind == WorkflowNodeKind::Review)
        {
            if node.gate.as_ref().and_then(|g| g.actor.as_ref()).is_none() {
                violations.push(format!(
                    "review {} has no independent functionary: declare another concrete task agent in scope {}",
                    node.id, injected.scope));
            }
        }
        Ok(WorkflowLint {
            subject: prepared.subject,
            scope: prepared.scope,
            plans: plans.into_values().collect(),
            injections,
            violations,
            injected: Some(injected),
            part: prepared.part,
        })
    }

    pub async fn bind_functionaries(&self, definition: &mut WorkflowDefinition) -> Result<()> {
        let roster = Facts::<L5>::new()
            .get::<FunctionaryRosterFact, _>(&self.roster, &definition.scope)
            .await?;
        let subjects: Vec<(String, String)> = definition
            .nodes
            .iter()
            .filter(|n| n.kind == WorkflowNodeKind::Task)
            .map(|n| {
                (
                    n.id.clone(),
                    n.task
                        .agent
                        .clone()
                        .or_else(|| roster.default_agent.clone())
                        .unwrap_or_else(|| self.default_agent.clone()),
                )
            })
            .collect();
        for node in &mut definition.nodes {
            let Some(spec) = node.gate.as_mut() else {
                continue;
            };
            let Some((_, executor)) = subjects
                .iter()
                .find(|(id, _)| spec.subject.as_deref() == Some(id))
            else {
                continue;
            };
            spec.actor = match node.kind {
                WorkflowNodeKind::Review => {
                    roster.names.iter().find(|name| *name != executor).cloned()
                }
                WorkflowNodeKind::Approval => Some("owner".into()),
                _ => spec.actor.clone(),
            };
        }
        Ok(())
    }
}
impl<R> FactProvider for Provider<R> {
    type Level = L5;
}
#[async_trait::async_trait]
impl<R: Send + Sync> Provide<WorkflowTargetsFact> for Provider<R> {
    type Query = WorkflowBlueprintFact;
    type Value = WorkflowTargetsFact;
    type Error = FactoryError;
    async fn get(&self, blueprint: &WorkflowBlueprintFact) -> Result<WorkflowTargetsFact> {
        let prepared =
            Self::prepare_definition(factory_process::workflow_blueprints::definition(blueprint)?)?;
        Ok(WorkflowTargetsFact {
            scope: prepared.scope,
            categories: prepared.categories,
        })
    }
}
#[async_trait::async_trait]
impl<R> Provide<WorkflowPreviewFact> for Provider<R>
where
    R: Provide<
        FunctionaryRosterFact,
        Query = String,
        Value = FunctionaryRosterFact,
        Error = FactoryError,
    >,
{
    type Query = Read;
    type Value = WorkflowPreviewFact;
    type Error = FactoryError;
    async fn get(&self, read: &Read) -> Result<WorkflowPreviewFact> {
        let prepared = Self::prepare_definition(factory_process::workflow_blueprints::definition(
            &read.blueprint,
        )?)?;
        // FactoryError is not cloneable: transport only its legacy message,
        // which is the observation Policy previously recorded on lint refusal.
        let requirements = read
            .requirements
            .iter()
            .map(|input| {
                input
                    .as_ref()
                    .cloned()
                    .map_err(|error| FactoryError::Other(anyhow::anyhow!(error.to_string())))
            })
            .collect();
        let lint = self.finish(prepared, requirements).await?;
        let authored = &read.blueprint.blueprint;
        let mut enforcement = Vec::new();
        if let Some(injected) = lint.injected.as_ref() {
            for injection in &lint.injections {
                let Some(control) = injected
                    .nodes
                    .iter()
                    .find(|node| node.id == injection.gate_node_id)
                else {
                    continue;
                };
                let Some(spec) = control.gate.as_ref() else {
                    continue;
                };
                let kind = match control.kind {
                    WorkflowNodeKind::Gate => StepKind::Gate,
                    WorkflowNodeKind::Review => StepKind::Review,
                    WorkflowNodeKind::Approval => StepKind::Approval,
                    WorkflowNodeKind::Task | WorkflowNodeKind::Expand => continue,
                };
                enforcement.push(WorkflowEnforcement {
                    workflow: authored.id.clone(),
                    name: authored.name.clone(),
                    scope: authored.scope.clone(),
                    node: injection.node_id.clone(),
                    step: injection.step.clone(),
                    kind,
                    required_by: injection.required_by.clone(),
                    actor: spec.actor.clone(),
                });
            }
        }
        let findings = lint
            .violations
            .into_iter()
            .map(|detail| WorkflowEnforcementFinding {
                workflow: authored.id.clone(),
                name: authored.name.clone(),
                scope: authored.scope.clone(),
                detail,
            })
            .collect();
        Ok(WorkflowPreviewFact {
            enforcement,
            findings,
        })
    }
}
