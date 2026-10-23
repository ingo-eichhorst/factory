//! When a run's agent needs a decision. With `daemon.orchestrator` set, the
//! question goes first to that agent, which answers it on the owner's
//! behalf within the run's mandate or escalates it -- see `on_blocked`.
//! Without one, or once it escalates, the question is the owner's
//! (`daemon.escalation`). Its question is in the Inbox either way. With a channel configured it is also sent
//! there, and a reply to that message -- or a press of one of its buttons --
//! answers it, through the same `task.answer` the Inbox and the CLI use: typed
//! into the session while it is open, resumed into a new attempt once it is
//! gone.
//!
//! What was sent is kept in the journal rather than a table of its own: an
//! `escalation_sent` entry on the run names the channel, the question, its
//! options and the message ids a reply can name. Whether the question is
//! still waiting is the journal's to say as well (`pending_question`), so an
//! answer from anywhere -- the Inbox, the CLI, a second reply -- settles it
//! for every other way of answering.

use crate::access::Caller;
use crate::l4_service::L4Service;
use chrono::{Duration, Utc};
use factory_core::config::OrchestratorConfig;
use factory_core::error::{FactoryError, Result};
use factory_core::operations;
use factory_core::protocol::Decision;
use factory_core::run::{BlockSource, Run, RunStatus};
use factory_core::task::{NewTask, Task, TaskEntry};
use std::collections::BTreeMap;
use factory_environment::channels::{self as ch, Button};

/// The journal kind of a question sent to the owner's channel.
pub(crate) const ESCALATION_SENT_KIND: &str = "escalation_sent";

/// How far back a reply or a button press may reach for its question.
const ANSWER_WINDOW_DAYS: i64 = 14;

/// How much of a run's journal is read for its question.
const QUESTION_ENTRIES: u32 = 50;

/// The labels that make a task a decision task: the task and the run whose
/// question it decides. Set by the daemon when it makes one.
pub(crate) const DECIDES_TASK: &str = "decides.task";
pub(crate) const DECIDES_RUN: &str = "decides.run";

/// On the blocked task: the orchestrator was asked, it answered, it
/// escalated, it gave no decision, or its decision came too late.
pub(crate) const ORCHESTRATOR_ASKED_KIND: &str = "orchestrator_asked";
pub(crate) const ORCHESTRATOR_DECIDED_KIND: &str = "orchestrator_decided";
pub(crate) const ORCHESTRATOR_ESCALATED_KIND: &str = "orchestrator_escalated";
const ORCHESTRATOR_SILENT_KIND: &str = "orchestrator_silent";
const DECISION_STALE_KIND: &str = "decision_stale";
/// On the decision task: what it decided, once. A second is refused.
const DECISION_KIND: &str = "decision";

/// How much of a blocked run the orchestrator is shown.
const SHOWN_ENTRIES: usize = 40;
const SHOWN_SCREEN_LINES: u32 = 150;
const SHOWN_INSTRUCTION_CHARS: usize = 6000;
const SHOWN_ENTRY_CHARS: usize = 400;

/// What the orchestrator decided, for its caller to carry out: an answer
/// goes through `task.answer`, which the router starts.
pub(crate) enum Decided {
    Answer { task: String, text: String },
    Escalated,
    /// The question was answered some other way first.
    Stale,
}

/// A question's buttons carry `ans:<run id>:<option>` -- under Telegram's
/// 64-byte limit for a uuid -- and at most this many fit a phone's row.
pub(crate) const ANSWER_BUTTON: &str = "ans";
const MAX_OPTIONS: usize = 4;

impl L4Service<'_> {
    /// The agent of `run_id` reported `blocked`. Its question goes to the
    /// orchestrator when one is set and may take it -- `Some` is the
    /// decision task the caller starts -- and otherwise to the owner.
    /// Called for every `blocked` report: the same question again starts
    /// and sends nothing new. Nothing here fails the report; the question
    /// is in the Inbox whatever happens.
    pub(crate) async fn on_blocked(&self, run_id: &str) -> Option<String> {
        match self.try_on_blocked(run_id).await {
            Ok(decision) => decision,
            Err(e) => {
                tracing::warn!(run = run_id, "could not pass a blocked run's question on: {e}");
                None
            }
        }
    }

    async fn try_on_blocked(&self, run_id: &str) -> Result<Option<String>> {
        let run = self.require_run(run_id).await?;
        // A hook's block is often a permission prompt nobody read, and a
        // verification hold is an approval or a gate: never the
        // orchestrator's, and nothing to send on.
        if run.status != RunStatus::Blocked || run.blocked_source != Some(BlockSource::Agent) {
            return Ok(None);
        }
        let entries = self.state.store.run_entries(&run.id, QUESTION_ENTRIES).await?;
        let Some(question) = operations::pending_question(&entries).map(str::to_string) else {
            return Ok(None);
        };
        let asked_before = entries
            .iter()
            .rev()
            .find(|e| e.kind == ORCHESTRATOR_ASKED_KIND)
            .and_then(|e| e.data.as_ref()?.get("question")?.as_str().map(str::to_string));
        if asked_before.as_deref() == Some(question.as_str()) {
            return Ok(None);
        }
        let task = self.require(&run.task_id).await?;
        match self.orchestrator_for(&task).await? {
            Ok(orchestrator) => Ok(Some(self.ask_orchestrator(&task, &run, &question, &orchestrator, &entries).await?)),
            Err(why) => {
                self.escalate_to_owner(&run, &question, why.as_deref(), &[]).await?;
                Ok(None)
            }
        }
    }

    /// The orchestrator that may decide `task`'s question, or why the owner
    /// is asked instead -- `None` when there is nothing worth saying.
    async fn orchestrator_for(&self, task: &Task) -> Result<std::result::Result<OrchestratorConfig, Option<String>>> {
        let snapshot = self.wiring.snapshot();
        let Some(orchestrator) = snapshot.config.daemon.orchestrator.clone().filter(|o| o.enabled) else {
            return Ok(Err(None));
        };
        // The orchestrator's own question, the owner's own conversation,
        // and a scope the owner keeps for themselves are the owner's.
        if task.labels.contains_key(DECIDES_RUN)
            || task.labels.contains_key(ch::LABEL_CHANNEL)
            || orchestrator.exclude.contains(&task.scope)
        {
            return Ok(Err(None));
        }
        let asked = self
            .state
            .store
            .entries(&task.id, 1000)
            .await?
            .iter()
            .filter(|e| e.kind == ORCHESTRATOR_ASKED_KIND)
            .count() as u32;
        if asked >= orchestrator.max_decisions_per_task {
            return Ok(Err(Some(format!(
                "The orchestrator has decided {asked} questions of this task already; this one is yours."
            ))));
        }
        Ok(Ok(orchestrator))
    }

    /// Make the decision task for `run`'s `question`, in the instance root's
    /// scope, for the orchestrator. The caller starts it.
    async fn ask_orchestrator(
        &self,
        task: &Task,
        run: &Run,
        question: &str,
        orchestrator: &OrchestratorConfig,
        entries: &[TaskEntry],
    ) -> Result<String> {
        let snapshot = self.wiring.snapshot();
        let root = snapshot
            .config
            .scope
            .as_ref()
            .ok_or_else(|| FactoryError::BadRequest("daemon.orchestrator needs the instance root's own scope:".into()))?;
        let screen = match &run.session {
            Some(session) => match self.wiring.registry().runtime(&session.runtime) {
                Ok(runtime) => runtime.read(session, SHOWN_SCREEN_LINES).await.ok(),
                Err(_) => None,
            },
            None => None,
        };
        let workdir = match &run.worktree_path {
            Some(path) => path.clone(),
            None => snapshot.scope_path(&task.scope)?.display().to_string(),
        };
        let policy = orchestrator.policy.as_ref().map(|p| snapshot.root.join(p).display().to_string());
        let shown = Shown { task, run, question, entries, screen: screen.as_deref(), workdir: &workdir, policy: policy.as_deref() };
        let mut labels = BTreeMap::new();
        labels.insert(DECIDES_TASK.to_string(), task.id.clone());
        labels.insert(DECIDES_RUN.to_string(), run.id.clone());
        let decision = self
            .create(NewTask {
                title: format!("Decide: {}", truncate(&task.title, 80)),
                instructions: decision_prompt(&shown),
                scope: Some(root.name.clone()),
                agent: Some(orchestrator.agent.clone()),
                worktree: Some(false),
                labels,
                ..Default::default()
            })
            .await?;
        self.entry(
            &task.id,
            TaskEntry::new("daemon", ORCHESTRATOR_ASKED_KIND, format!("the orchestrator was asked to decide (task {})", decision.id))
                .in_run(&run.id)
                .with_data(serde_json::json!({ "question": question, "decision_task": decision.id, "agent": orchestrator.agent })),
        )
        .await;
        Ok(decision.id)
    }

    /// `decision.resolve`: the orchestrator's decision on the question its
    /// decision task `id` was made for. Once per decision task. A decision
    /// on a question answered some other way meanwhile is journaled and
    /// dropped.
    pub(crate) async fn resolve_decision(&self, id: &str, decision: Decision) -> Result<Decided> {
        let decider = self.require(id).await?;
        let (Some(task_id), Some(run_id)) = (decider.labels.get(DECIDES_TASK), decider.labels.get(DECIDES_RUN)) else {
            return Err(FactoryError::BadRequest(format!("task {id} is not deciding anyone's question")));
        };
        let (task_id, run_id) = (task_id.clone(), run_id.clone());
        if self.state.store.entries(id, 200).await?.iter().any(|e| e.kind == DECISION_KIND) {
            return Err(FactoryError::BadRequest(format!("task {id} has decided already")));
        }
        let (kind, said) = match &decision {
            Decision::Answer { text, reason } => ("answer", format!("answered: {text} -- {reason}")),
            Decision::Escalate { reason, .. } => ("escalate", format!("escalated: {reason}")),
        };
        self.entry(id, TaskEntry::new("agent", DECISION_KIND, said).with_data(serde_json::json!({ "decision": kind }))).await;

        if self.still_asking(&task_id, &run_id).await.is_err() {
            self.entry(
                &task_id,
                TaskEntry::new("daemon", DECISION_STALE_KIND, format!("the orchestrator's decision (task {id}) came after the question was answered; dropped"))
                    .in_run(&run_id),
            )
            .await;
            return Ok(Decided::Stale);
        }
        match decision {
            Decision::Answer { text, reason } => {
                if text.trim().is_empty() || reason.trim().is_empty() {
                    return Err(FactoryError::BadRequest("a decision needs its answer and its reason".into()));
                }
                // The text is recorded: unlike a person's answer, it is a
                // decision made on the owner's behalf, and the owner reads
                // what was decided and why.
                self.entry(
                    &task_id,
                    TaskEntry::new("agent", ORCHESTRATOR_DECIDED_KIND, format!("the orchestrator answered: {text}"))
                        .in_run(&run_id)
                        .with_data(serde_json::json!({ "decision_task": id, "answer": text, "reason": reason })),
                )
                .await;
                Ok(Decided::Answer { task: task_id, text })
            }
            Decision::Escalate { reason, options } => {
                let options: Vec<String> = options.into_iter().filter(|o| !o.trim().is_empty()).take(MAX_OPTIONS).collect();
                self.entry(
                    &task_id,
                    TaskEntry::new("agent", ORCHESTRATOR_ESCALATED_KIND, format!("the orchestrator escalated to the owner: {reason}"))
                        .in_run(&run_id)
                        .with_data(serde_json::json!({ "decision_task": id, "reason": reason, "options": options })),
                )
                .await;
                let run = self.require_run(&run_id).await?;
                let entries = self.state.store.run_entries(&run_id, QUESTION_ENTRIES).await?;
                let question = operations::pending_question(&entries).unwrap_or_default().to_string();
                self.escalate_to_owner(&run, &question, Some(&format!("The orchestrator escalated it: {reason}")), &options).await?;
                Ok(Decided::Escalated)
            }
        }
    }

    /// Called for every run that ends: a decision task that ended without a
    /// decision -- failed, timed out, cancelled, or done without deciding --
    /// leaves its question with the owner.
    pub(crate) async fn settle_decision(&self, run: &Run) {
        let Ok(Some(decider)) = self.state.store.get(&run.task_id).await else { return };
        let (Some(task_id), Some(run_id)) = (decider.labels.get(DECIDES_TASK), decider.labels.get(DECIDES_RUN)) else {
            return;
        };
        let decided = match self.state.store.entries(&decider.id, 200).await {
            Ok(entries) => entries.iter().any(|e| e.kind == DECISION_KIND),
            Err(_) => return,
        };
        if decided || self.still_asking(task_id, run_id).await.is_err() {
            return;
        }
        let why = run.error.clone().unwrap_or_else(|| format!("its run ended {} without a decision", run.status.as_str()));
        self.entry(
            task_id,
            TaskEntry::new("daemon", ORCHESTRATOR_SILENT_KIND, format!("the orchestrator gave no decision: {why}"))
                .in_run(run_id)
                .with_data(serde_json::json!({ "decision_task": decider.id })),
        )
        .await;
        let Ok(blocked) = self.require_run(run_id).await else { return };
        let Ok(entries) = self.state.store.run_entries(run_id, QUESTION_ENTRIES).await else { return };
        let question = operations::pending_question(&entries).unwrap_or_default().to_string();
        if let Err(e) = self.escalate_to_owner(&blocked, &question, Some(&format!("The orchestrator gave no decision: {why}")), &[]).await {
            tracing::warn!(run = run_id.as_str(), "could not send a question on to the owner: {e}");
        }
    }

    /// Send `question` to the owner's channel, with `why` a person is being
    /// asked and up to four `options` as buttons. Nothing to do without a
    /// channel; the Inbox already has it. Sent once per question per run.
    pub(crate) async fn escalate_to_owner(&self, run: &Run, question: &str, why: Option<&str>, options: &[String]) -> Result<()> {
        let Some(channel) = self.wiring.snapshot().config.daemon.escalation.channel.clone() else {
            return Ok(());
        };
        let entries = self.state.store.run_entries(&run.id, QUESTION_ENTRIES).await?;
        let sent_before = entries
            .iter()
            .rev()
            .find(|e| e.kind == ESCALATION_SENT_KIND)
            .and_then(|e| e.data.as_ref()?.get("question")?.as_str().map(str::to_string));
        if sent_before.as_deref() == Some(question) {
            return Ok(());
        }
        let task = self.require(&run.task_id).await?;
        // A task that came in on this channel is a conversation already:
        // its question goes into that thread.
        let reply_to = match task.labels.get(ch::LABEL_CHANNEL) {
            Some(came_in) if *came_in == channel => task.labels.get(ch::LABEL_MESSAGE).and_then(|m| m.parse().ok()),
            _ => None,
        };
        let options: Vec<String> = options.iter().take(MAX_OPTIONS).cloned().collect();
        let notified = self
            .notify(&Caller::Owner, &channel, notice(&task.title, &task.scope, &task.id, run.attempt, question, why), Vec::new(), reply_to, buttons(&run.id, &options))
            .await?;
        self.entry(
            &task.id,
            TaskEntry::new("daemon", ESCALATION_SENT_KIND, format!("the question went to the owner on {channel}"))
                .in_run(&run.id)
                .with_data(serde_json::json!({
                    "channel": channel,
                    "question": question,
                    "options": options,
                    "message_ids": notified.message_ids,
                })),
        )
        .await;
        Ok(())
    }

    /// The task and run whose question a reply on `channel` to `message`
    /// answers. `TaskNotFound` when that message carried none -- the
    /// gateway then takes the reply as any other message.
    pub(crate) async fn escalation_for_reply(&self, channel: &str, message: i64) -> Result<(String, String)> {
        let since = Utc::now() - Duration::days(ANSWER_WINDOW_DAYS);
        let sent = self.state.store.entries_of_kinds(&[ESCALATION_SENT_KIND], since).await?;
        sent.into_iter()
            .rev()
            .find(|(_, entry)| {
                let data = entry.data.as_ref();
                data.and_then(|d| d.get("channel")?.as_str()) == Some(channel)
                    && data
                        .and_then(|d| d.get("message_ids")?.as_array().cloned())
                        .is_some_and(|ids| ids.iter().any(|id| id.as_i64() == Some(message)))
            })
            .and_then(|(task_id, entry)| Some((task_id, entry.run_id?)))
            .ok_or_else(|| FactoryError::TaskNotFound(format!("a question sent as message {message} on {channel}")))
    }

    /// The task of `run_id`, and the text of the `option` its question was
    /// sent on `channel` with.
    pub(crate) async fn escalation_option(&self, channel: &str, run_id: &str, option: usize) -> Result<(String, String)> {
        let since = Utc::now() - Duration::days(ANSWER_WINDOW_DAYS);
        let sent = self.state.store.entries_of_kinds(&[ESCALATION_SENT_KIND], since).await?;
        let (task_id, entry) = sent
            .into_iter()
            .rev()
            .find(|(_, entry)| {
                entry.run_id.as_deref() == Some(run_id)
                    && entry.data.as_ref().and_then(|d| d.get("channel")?.as_str()) == Some(channel)
            })
            .ok_or_else(|| FactoryError::TaskNotFound(format!("a question sent for run {run_id} on {channel}")))?;
        let text = entry
            .data
            .as_ref()
            .and_then(|d| d.get("options")?.get(option)?.as_str().map(str::to_string))
            .ok_or_else(|| FactoryError::BadRequest(format!("that question has no option {}", option + 1)))?;
        Ok((task_id, text))
    }

    /// Refused unless `run_id`'s question is still the one its task waits
    /// on: its newest attempt, nothing answered or said since. A reply to a
    /// question that was answered elsewhere is told so, rather than typed
    /// into whatever the agent is doing now.
    pub(crate) async fn still_asking(&self, task_id: &str, run_id: &str) -> Result<()> {
        let newest = self.state.store.runs(task_id, 1).await?.into_iter().next();
        let entries = self.state.store.run_entries(run_id, QUESTION_ENTRIES).await?;
        if newest.as_ref().map(|r| r.id.as_str()) != Some(run_id) || operations::pending_question(&entries).is_none() {
            return Err(FactoryError::BadRequest("that question has been answered already".into()));
        }
        Ok(())
    }
}

/// What a decision run is shown of the run it decides for.
struct Shown<'a> {
    task: &'a Task,
    run: &'a Run,
    question: &'a str,
    entries: &'a [TaskEntry],
    screen: Option<&'a str>,
    workdir: &'a str,
    policy: Option<&'a str>,
}

/// A decision task's instructions: the question and everything the
/// orchestrator needs to decide it, and the two ways it may.
fn decision_prompt(shown: &Shown<'_>) -> String {
    let Shown { task, run, question, entries, screen, workdir, policy } = shown;
    let mut out = String::from(
        "A Factory run is blocked on a question. Decide it on the owner's behalf, or escalate it to the owner. \
         You do not do the run's work, and you change nothing in its working directory.\n\n",
    );
    if let Some(policy) = policy {
        out.push_str(&format!("Read the decision policy first: {policy}\n\n"));
    }
    out.push_str(&format!(
        "The question, from attempt {} of task {} \"{}\" (scope {}, agent {}):\n\n{question}\n\n",
        run.attempt, task.id, task.title, task.scope, task.agent
    ));
    out.push_str(&format!("The task's instructions -- its mandate:\n\n{}\n\n", truncate(&task.instructions, SHOWN_INSTRUCTION_CHARS)));
    out.push_str("What the run has recorded, newest last:\n\n");
    let skip = entries.len().saturating_sub(SHOWN_ENTRIES);
    for entry in &entries[skip..] {
        out.push_str(&format!(
            "- {} {} {}: {}\n",
            entry.at.format("%Y-%m-%d %H:%M"),
            entry.source,
            entry.kind,
            truncate(&entry.message, SHOWN_ENTRY_CHARS)
        ));
    }
    if let Some(screen) = screen.filter(|s| !s.trim().is_empty()) {
        out.push_str(&format!("\nThe end of its screen:\n\n{screen}\n"));
    }
    out.push_str(&format!(
        "\nIts working directory, to read: {workdir}\n\n\
         Decide with exactly one of these, then report done:\n\n\
         \x20 factory decision answer \"<the answer, as the blocked agent should read it>\" --reason \"<why, with the evidence>\"\n\
         \x20 factory decision escalate --reason \"<why the owner must decide>\" --option \"<choice>\" --option \"<choice>\"\n\n\
         Escalate anything that would widen what the task is allowed to do -- merging outside its stated rule, \
         releasing, publishing, sending, spending, deleting -- and anything where reasonable owners could choose \
         differently and a wrong guess is costly. Give the owner two to four short options.\n"
    ));
    out
}

fn truncate(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let kept: String = text.chars().take(chars).collect();
    format!("{kept} [...]")
}

/// The message a question goes out as. Says which task and attempt, so two
/// questions that read the same are still two messages -- the channel skips
/// an identical one sent within 12 hours.
fn notice(title: &str, scope: &str, task_id: &str, attempt: u32, question: &str, why: Option<&str>) -> String {
    let mut text = format!("❓ {title} ({scope}) is waiting for your answer:\n\n{question}");
    if let Some(why) = why {
        text.push_str(&format!("\n\n{why}"));
    }
    let short: String = task_id.chars().take(8).collect();
    text.push_str(&format!("\n\nReply to this message to answer it. · task {short}, attempt {attempt}"));
    text
}

/// One button per option, each answering with its own text.
fn buttons(run_id: &str, options: &[String]) -> Vec<Button> {
    options
        .iter()
        .enumerate()
        .map(|(i, text)| Button { text: text.clone(), data: format!("{ANSWER_BUTTON}:{run_id}:{i}") })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_names_its_task_and_attempt_so_a_repeated_question_is_still_sent() {
        let first = notice("Irrlicht #2095", "irrlicht", "8d67bbf6-43d6", 1, "merge as-is?", None);
        let second = notice("Irrlicht #2095", "irrlicht", "8d67bbf6-43d6", 2, "merge as-is?", Some("the orchestrator escalated it"));
        assert!(first.contains("merge as-is?") && first.contains("task 8d67bbf6, attempt 1"), "{first}");
        assert!(second.contains("the orchestrator escalated it"), "{second}");
        assert_ne!(first, second);
    }

    #[test]
    fn a_button_fits_telegrams_callback_data() {
        let run = "5731d80a-1f5f-486a-b3f5-3f54d3588b3d";
        let made = buttons(run, &["Merge as-is".into(), "Suppress, then merge".into()]);
        assert_eq!(made[1].data, format!("ans:{run}:1"));
        assert!(made.iter().all(|b| b.data.len() <= 64));
    }
}

#[cfg(test)]
mod orchestrator_tests {
    use super::*;
    use crate::engine::Engine;
    use factory_core::adapter::{AgentRuntime, StartRequest};
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_core::protocol::{Payload, Request, Response};
    use factory_core::run::{FailKind, NewRun, RunPatch, Trigger};
    use factory_core::task::SessionRef;
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    /// A runtime that opens nothing and remembers what was typed into it.
    #[derive(Default)]
    struct Recorder {
        typed: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl AgentRuntime for Recorder {
        fn name(&self) -> &str {
            "quiet"
        }
        async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
            Ok(SessionRef { runtime: "quiet".into(), handle: req.id.clone(), meta: Default::default() })
        }
        async fn submit(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn status(&self, _: &SessionRef) -> Result<factory_core::adapter::RuntimeStatus> {
            Ok(factory_core::adapter::RuntimeStatus::Working)
        }
        async fn send_text(&self, _: &SessionRef, text: &str) -> Result<()> {
            self.typed.lock().unwrap().push(text.to_string());
            Ok(())
        }
        async fn send_keys(&self, _: &SessionRef, _: &[String]) -> Result<()> {
            Ok(())
        }
        async fn read(&self, _: &SessionRef, _: u32) -> Result<String> {
            Ok("the PR is MERGEABLE but UNSTABLE: CodeScene is red".into())
        }
        async fn stop(&self, _: &SessionRef) -> Result<()> {
            Ok(())
        }
    }

    fn scope(yaml: &str, path: PathBuf) -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(yaml).unwrap();
        scope.path = path;
        scope
    }

    /// A root scope declaring `orchestrator`, the project scope `demo` and
    /// `assistant` (excluded), with `daemon.orchestrator` set.
    fn engine(max_decisions_per_task: u32) -> (Arc<Engine>, Arc<Recorder>, PathBuf) {
        let root = std::env::temp_dir().join(format!("factory-orchestrator-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("projects/demo")).unwrap();
        std::fs::create_dir_all(root.join("company/assistant")).unwrap();
        let top = scope("id: root-id\nname: root\nruntime: quiet\nagents:\n  - name: orchestrator\n    harness: shell\n", root.clone());
        let config = Config {
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig {
                power_assertion: false,
                orchestrator: Some(OrchestratorConfig {
                    enabled: true,
                    agent: "orchestrator".into(),
                    policy: Some(".factory/orchestrator.md".into()),
                    max_decisions_per_task,
                    exclude: vec!["assistant".into()],
                }),
                ..DaemonConfig::default()
            },
            scope: Some(top.clone()),
            scopes: vec![
                top,
                scope("id: demo-id\nname: demo\nruntime: quiet\n", root.join("projects/demo")),
                scope("id: assistant-id\nname: assistant\nruntime: quiet\n", root.join("company/assistant")),
            ],
            ..Default::default()
        };
        config.validate().unwrap();
        let runtime = Arc::new(Recorder::default());
        let mut registry = Registry::with_builtins();
        registry.add_runtime(runtime.clone(), "test");
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        let engine = Arc::new(Engine::new(Factory { root: root.clone(), config }, registry, store, PathBuf::from("factory"), Vec::new()));
        (engine, runtime, root)
    }

    /// A task in `scope` whose run's agent reported `blocked` with `question`.
    async fn blocked(engine: &Engine, scope: &str, question: &str, source: BlockSource) -> (Task, Run) {
        let task = engine
            .l4_service()
            .create(NewTask {
                title: "Merge PR 2097".into(),
                instructions: "Merge only when the PR is CLEAN.".into(),
                scope: Some(scope.into()),
                agent: Some("shell".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let run = engine
            .l4
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "quiet".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        let session = SessionRef { runtime: "quiet".into(), handle: run.id.clone(), meta: Default::default() };
        let run = engine
            .l4
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Blocked),
                    session: Some(session),
                    blocked_since: Some(Utc::now()),
                    blocked_source: Some(source),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        engine.l4_service().entry(&task.id, TaskEntry::new("agent", "blocked", question).in_run(&run.id)).await;
        (task, run)
    }

    async fn kinds(engine: &Engine, task: &str) -> Vec<String> {
        engine.l4.store.entries(task, 200).await.unwrap().into_iter().map(|e| e.kind).collect()
    }

    #[tokio::test]
    async fn a_question_goes_to_the_orchestrator_with_what_it_needs_to_decide() {
        let (engine, _, root) = engine(3);
        let (task, run) = blocked(&engine, "demo", "merge as-is or suppress in CodeScene?", BlockSource::Agent).await;

        let decision = engine.l4_service().on_blocked(&run.id).await.expect("a decision task");
        let decider = engine.l4_service().require(&decision).await.unwrap();
        assert_eq!(decider.scope, "root");
        assert_eq!(decider.agent, "orchestrator");
        assert_eq!(decider.labels.get(DECIDES_RUN), Some(&run.id));
        assert_eq!(decider.labels.get(DECIDES_TASK), Some(&task.id));
        let prompt = &decider.instructions;
        for shown in ["merge as-is or suppress in CodeScene?", "Merge only when the PR is CLEAN.", "UNSTABLE", "orchestrator.md", "factory decision answer", "factory decision escalate"] {
            assert!(prompt.contains(shown), "the prompt shows {shown:?}:\n{prompt}");
        }
        assert!(kinds(&engine, &task.id).await.contains(&ORCHESTRATOR_ASKED_KIND.to_string()));

        assert!(engine.l4_service().on_blocked(&run.id).await.is_none(), "the same question is not asked twice");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn an_answer_decided_is_typed_into_the_blocked_session_and_recorded_with_its_reason() {
        let (engine, runtime, root) = engine(3);
        let (task, run) = blocked(&engine, "demo", "use postgres or sqlite?", BlockSource::Agent).await;
        let decision = engine.l4_service().on_blocked(&run.id).await.unwrap();

        let answer = factory_core::protocol::Decision::Answer { text: "postgres".into(), reason: "the repo already runs it".into() };
        match engine.handle_request(Request::DecisionResolve { id: decision.clone(), decision: answer.clone() }).await {
            Response::Ok { data: Payload::Text { text } } => assert!(text.contains("typed into attempt 1"), "{text}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(runtime.typed.lock().unwrap().first().map(String::as_str), Some("postgres"));
        let decided = engine.l4.store.entries(&task.id, 200).await.unwrap().into_iter().find(|e| e.kind == ORCHESTRATOR_DECIDED_KIND).unwrap();
        assert_eq!(decided.data.as_ref().unwrap()["answer"], "postgres", "a decision on the owner's behalf is on the record");
        assert_eq!(decided.data.as_ref().unwrap()["reason"], "the repo already runs it");

        let again = engine.l4_service().resolve_decision(&decision, answer).await.err().unwrap();
        assert!(again.to_string().contains("decided already"), "{again}");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn an_escalation_reaches_the_inbox_with_its_reason_and_options() {
        let (engine, runtime, root) = engine(3);
        let (task, run) = blocked(&engine, "demo", "merge while CodeScene is red?", BlockSource::Agent).await;
        let decision = engine.l4_service().on_blocked(&run.id).await.unwrap();
        let escalate = factory_core::protocol::Decision::Escalate {
            reason: "merging outside the task's CLEAN rule widens its mandate".into(),
            options: vec!["Merge as-is".into(), "Suppress, then merge".into()],
        };
        assert!(matches!(engine.l4_service().resolve_decision(&decision, escalate).await.unwrap(), Decided::Escalated));
        assert!(runtime.typed.lock().unwrap().is_empty(), "nothing is answered");

        let report = engine.operations_report(None, factory_core::operations::HealthWindow::Week, false).await.unwrap();
        let item = report.attention.iter().find(|e| e.task_id.as_deref() == Some(task.id.as_str())).unwrap();
        assert_eq!(item.options, vec!["Merge as-is".to_string(), "Suppress, then merge".to_string()]);
        assert!(item.reason.contains("merge while CodeScene is red?"), "{}", item.reason);
        assert!(item.reason.contains("the orchestrator escalated it: merging outside"), "{}", item.reason);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn some_questions_never_reach_the_orchestrator() {
        let (engine, _, root) = engine(1);
        // A hook's block: perhaps a permission prompt nobody read.
        let (_, hooked) = blocked(&engine, "demo", "Allow this command?", BlockSource::Runtime).await;
        assert!(engine.l4_service().on_blocked(&hooked.id).await.is_none());
        // A scope the owner keeps for themselves.
        let (_, mine) = blocked(&engine, "assistant", "which restaurant?", BlockSource::Agent).await;
        assert!(engine.l4_service().on_blocked(&mine.id).await.is_none());

        // Past the cap, the next question is the owner's.
        let (task, run) = blocked(&engine, "demo", "first?", BlockSource::Agent).await;
        assert!(engine.l4_service().on_blocked(&run.id).await.is_some());
        engine.l4_service().entry(&task.id, TaskEntry::new("agent", "blocked", "second?").in_run(&run.id)).await;
        assert!(engine.l4_service().on_blocked(&run.id).await.is_none(), "max_decisions_per_task: 1");
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_decision_run_that_ends_without_deciding_leaves_the_question_with_the_owner() {
        let (engine, _, root) = engine(3);
        let (task, run) = blocked(&engine, "demo", "which region?", BlockSource::Agent).await;
        let decision = engine.l4_service().on_blocked(&run.id).await.unwrap();
        let decision_run = engine
            .l4
            .store
            .create_run(&NewRun {
                task_id: decision.clone(),
                trigger: Trigger::Manual,
                agent: "orchestrator".into(),
                adapter: "shell".into(),
                runtime: "quiet".into(),
                token: "tok2".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine.l4_service().fail_run(&decision_run.id, FailKind::RunTimeout, "ran out of time").await;
        assert!(kinds(&engine, &task.id).await.contains(&"orchestrator_silent".to_string()));
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_decision_on_a_question_answered_meanwhile_is_dropped() {
        let (engine, runtime, root) = engine(3);
        let (task, run) = blocked(&engine, "demo", "which region?", BlockSource::Agent).await;
        let decision = engine.l4_service().on_blocked(&run.id).await.unwrap();
        engine.l4_service().entry(&task.id, TaskEntry::new("owner", "answer", "answered").in_run(&run.id)).await;
        let answer = factory_core::protocol::Decision::Answer { text: "eu".into(), reason: "closest".into() };
        assert!(matches!(engine.l4_service().resolve_decision(&decision, answer).await.unwrap(), Decided::Stale));
        assert!(runtime.typed.lock().unwrap().is_empty());
        assert!(kinds(&engine, &task.id).await.contains(&"decision_stale".to_string()));
        std::fs::remove_dir_all(root).ok();
    }
}
