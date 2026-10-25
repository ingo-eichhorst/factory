//! When a run's agent needs the owner (`daemon.escalation`). Its question
//! is in the Inbox either way. With a channel configured it is also sent
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
use factory_core::error::{FactoryError, Result};
use factory_core::operations;
use factory_core::run::{BlockSource, Run, RunStatus};
use factory_core::task::TaskEntry;
use factory_environment::channels::{self as ch, Button};

/// The journal kind of a question sent to the owner's channel.
pub(crate) const ESCALATION_SENT_KIND: &str = "escalation_sent";

/// How far back a reply or a button press may reach for its question.
const ANSWER_WINDOW_DAYS: i64 = 14;

/// How much of a run's journal is read for its question.
const QUESTION_ENTRIES: u32 = 50;

/// A question's buttons carry `ans:<run id>:<option>` -- under Telegram's
/// 64-byte limit for a uuid -- and at most this many fit a phone's row.
pub(crate) const ANSWER_BUTTON: &str = "ans";
const MAX_OPTIONS: usize = 4;

impl L4Service<'_> {
    /// The agent of `run_id` reported `blocked`: send its question on to the
    /// owner. Called for every `blocked` report -- an agent saying it again
    /// sends nothing new, a different question is sent. Nothing here fails
    /// the report: a send that does not go is logged and journaled by
    /// `notify`, and the question is still in the Inbox.
    pub(crate) async fn escalate_block(&self, run_id: &str) {
        if let Err(e) = self.try_escalate_block(run_id).await {
            tracing::warn!(run = run_id, "could not send a blocked run's question on: {e}");
        }
    }

    async fn try_escalate_block(&self, run_id: &str) -> Result<()> {
        let run = self.require_run(run_id).await?;
        if run.status != RunStatus::Blocked || run.blocked_source != Some(BlockSource::Agent) {
            return Ok(());
        }
        let entries = self.state.store.run_entries(&run.id, QUESTION_ENTRIES).await?;
        let Some(question) = operations::pending_question(&entries).map(str::to_string) else {
            return Ok(());
        };
        self.escalate_to_owner(&run, &question, None, &[]).await
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
