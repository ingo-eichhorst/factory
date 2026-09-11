//! Plugin-backed adapters. Each one implements a core trait by forwarding to a
//! child process, so the daemon cannot tell a plugin from a built-in.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use factory_core::adapter::agent::{Agent, AgentContext, LaunchKind, LaunchSpec};
use factory_core::adapter::store::TaskStore;
use factory_core::error::{FactoryError, Result};
use factory_core::agent::AgentSession;
use factory_core::run::{NewRun, Run, RunPatch};
use factory_core::task::{Task, TaskEntry, TaskFilter, TaskPatch};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::host::PluginProcess;

/// The agent context as a plugin sees it. Kept as its own type so the shape on
/// the wire is deliberate rather than whatever the internal struct happens to
/// look like this week.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WireContext {
    scope: String,
    cwd: String,
    factory_bin: String,
    socket: String,
    /// `null` when the agent is being started to stand there rather than to do
    /// something -- a plugin that only handles tasks should say so.
    #[serde(skip_serializing_if = "Option::is_none")]
    task: Option<Task>,
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempt: Option<u32>,
    /// The token travels to the plugin because the plugin is what puts it in
    /// front of the agent; it is stripped from everything else.
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    /// The same contract the built-in agents put in their prompts, so a plugin
    /// can paste it instead of reconstructing the commands.
    reporting_contract: String,
    env: std::collections::BTreeMap<String, String>,
}

impl From<&AgentContext> for WireContext {
    fn from(ctx: &AgentContext) -> Self {
        Self {
            scope: ctx.scope.clone(),
            cwd: ctx.cwd.display().to_string(),
            factory_bin: ctx.factory_bin.display().to_string(),
            socket: ctx.socket.display().to_string(),
            task: ctx.task.as_ref().map(|b| b.task.clone()),
            run_id: ctx.task.as_ref().map(|b| b.run_id.clone()),
            attempt: ctx.task.as_ref().map(|b| b.attempt),
            token: ctx.task.as_ref().map(|b| b.token.clone()),
            reporting_contract: ctx.reporting_contract(),
            env: ctx.env(),
        }
    }
}

pub struct PluginAgent {
    proc: Arc<PluginProcess>,
}

impl PluginAgent {
    pub fn new(proc: Arc<PluginProcess>) -> Self {
        Self { proc }
    }
}

#[async_trait]
impl Agent for PluginAgent {
    fn name(&self) -> &str {
        self.proc.name()
    }

    fn description(&self) -> String {
        self.proc.description()
    }

    async fn launch_spec(&self, ctx: &AgentContext) -> Result<LaunchSpec> {
        let wire = WireContext::from(ctx);
        let reply = self
            .proc
            .call_raw("agent.launch_spec", serde_json::to_value(&wire).unwrap_or(Value::Null))
            .await?;
        // A plugin that only wants to name a harness may answer with a bare
        // string; both spellings mean the same thing.
        if let Some(name) = reply.as_str() {
            return Ok(LaunchSpec {
                kind: LaunchKind::Named(name.to_string()),
                args: Vec::new(),
                env: wire.env,
            });
        }
        let mut spec: LaunchSpec = serde_json::from_value(reply).map_err(|e| {
            FactoryError::adapter(self.name(), format!("launch_spec reply is not a launch spec: {e}"))
        })?;
        // The callback environment is Factory's to set, not the plugin's to
        // forget: merge ours in without overwriting what the plugin added.
        for (k, v) in wire.env {
            spec.env.entry(k).or_insert(v);
        }
        Ok(spec)
    }

    async fn prompt(&self, ctx: &AgentContext) -> Result<String> {
        let wire = WireContext::from(ctx);
        let reply = self
            .proc
            .call_raw("agent.prompt", serde_json::to_value(&wire).unwrap_or(Value::Null))
            .await?;
        if let Some(text) = reply.as_str() {
            return Ok(text.to_string());
        }
        reply
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                FactoryError::adapter(self.name(), "prompt reply carried no `prompt` string")
            })
    }
}

pub struct PluginStore {
    proc: Arc<PluginProcess>,
}

impl PluginStore {
    pub fn new(proc: Arc<PluginProcess>) -> Self {
        Self { proc }
    }
}

#[async_trait]
impl TaskStore for PluginStore {
    fn name(&self) -> &str {
        self.proc.name()
    }

    fn description(&self) -> String {
        self.proc.description()
    }

    async fn create(&self, task: &Task) -> Result<Task> {
        self.proc.call("task.create", json!({ "task": task })).await
    }

    async fn get(&self, id: &str) -> Result<Option<Task>> {
        self.proc.call("task.get", json!({ "id": id })).await
    }

    async fn list(&self, filter: &TaskFilter) -> Result<Vec<Task>> {
        self.proc.call("task.list", json!({ "filter": filter })).await
    }

    async fn update(&self, id: &str, patch: &TaskPatch) -> Result<Task> {
        self.proc
            .call("task.update", json!({ "id": id, "patch": patch }))
            .await
    }

    async fn delete(&self, id: &str) -> Result<bool> {
        self.proc.call("task.delete", json!({ "id": id })).await
    }

    async fn create_run(&self, new: &NewRun) -> Result<Run> {
        self.proc.call("run.create", json!({ "run": new })).await
    }

    async fn get_run(&self, id: &str) -> Result<Option<Run>> {
        self.proc.call("run.get", json!({ "id": id })).await
    }

    async fn update_run(&self, id: &str, patch: &RunPatch) -> Result<Run> {
        self.proc
            .call("run.update", json!({ "id": id, "patch": patch }))
            .await
    }

    async fn runs(&self, task_id: &str, limit: u32) -> Result<Vec<Run>> {
        self.proc
            .call("run.list", json!({ "task_id": task_id, "limit": limit }))
            .await
    }

    async fn active_run(&self, task_id: &str) -> Result<Option<Run>> {
        self.proc
            .call("run.active", json!({ "task_id": task_id }))
            .await
    }

    async fn active_runs(&self) -> Result<Vec<Run>> {
        self.proc.call("run.active_all", json!({})).await
    }

    async fn runs_between(
        &self,
        from: chrono::DateTime<chrono::Utc>,
        to: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<Run>> {
        self.proc
            .call("run.between", json!({ "from": from, "to": to }))
            .await
    }

    async fn put_agent(&self, agent: &AgentSession) -> Result<()> {
        self.proc
            .call_raw("agent.put", json!({ "agent": agent }))
            .await
            .map(|_| ())
    }

    async fn get_agent(&self, id: &str) -> Result<Option<AgentSession>> {
        self.proc.call("agent.get", json!({ "id": id })).await
    }

    async fn agents(&self) -> Result<Vec<AgentSession>> {
        self.proc.call("agent.list", json!({})).await
    }

    async fn delete_agent(&self, id: &str) -> Result<bool> {
        self.proc.call("agent.delete", json!({ "id": id })).await
    }

    async fn append_entry(&self, id: &str, entry: &TaskEntry) -> Result<()> {
        self.proc
            .call_raw("task.append_entry", json!({ "id": id, "entry": entry }))
            .await
            .map(|_| ())
    }

    async fn entries(&self, id: &str, limit: u32) -> Result<Vec<TaskEntry>> {
        self.proc
            .call("task.entries", json!({ "id": id, "limit": limit }))
            .await
    }

    async fn run_entries(&self, run_id: &str, limit: u32) -> Result<Vec<TaskEntry>> {
        self.proc
            .call("run.entries", json!({ "run_id": run_id, "limit": limit }))
            .await
    }

    async fn due(&self, now: DateTime<Utc>) -> Result<Vec<Task>> {
        self.proc
            .call("task.due", json!({ "now": now.to_rfc3339() }))
            .await
    }
}
