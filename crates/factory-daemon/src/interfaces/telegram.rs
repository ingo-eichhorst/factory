//! The `telegram` interface (`#338`): how the owner reaches the daemon from
//! Telegram, through a channel's `inbound:` block.
//!
//! An interface like the socket and HTTP: everything that comes in is
//! translated into the one request envelope and handed to `Engine::handle`
//! as the owner -- there is no second API here, and nothing in this file
//! reaches a level's state or service. So a job run from the chat is
//! `task.run`, a reschedule is `task.update`, a message is `task.create`, and
//! each is journaled by its own handler as the owner's, the way the CLI's
//! are, with "on Telegram" as the reason.
//!
//! - `channel.poll` is the one `getUpdates` for the channel's bot token. L2
//!   holds the token for this gateway alone, applies the allowlist and
//!   fetches files; this side only sees what was let in.
//! - Something ignored -- a stranger, another chat -- is journaled with
//!   `channel.record` and never answered.
//! - `/jobs`, `/run <task>` and `/schedule <task> <cron> [timezone]` are the
//!   owner's commands. A reschedule, and anything the assistant proposes, is
//!   asked first with buttons ("Ja"/"Nein", or the words) and done only on a
//!   yes. `/intake <text>` hands a request to intake, as `chat` from
//!   provider `telegram`.
//! - Anything else is a task for the inbound scope's agent, approved to
//!   answer on this channel (`deliver`). It is acknowledged at once and
//!   answered when its run ends, however long that takes -- the run's own
//!   timeouts are the only clock. Waiting answers survive a restart: the
//!   task carries the message it answers, and a label once it was answered.
//!
//! What is pending a confirmation lives only in memory, for ten minutes: a
//! restart forgets it, and a button pressed after that says it expired.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use factory_core::adapter::interface::{Interface, InterfaceContext};
use factory_core::error::{FactoryError, Result};
use factory_core::intake::{NewIntake, SourceKind};
use factory_core::protocol::{Envelope, InboundKind, InboundRecord, Payload, Request, Response};
use factory_core::task::{CronSchedule, NewTask, Schedule, Task, TaskFilter, TaskPatch, TaskStatus};
use factory_environment::channels::{self as ch, Button, InboundDecl, Language, ReceivedFile};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::engine::Engine;

/// How long a question waits for its yes.
const PROPOSAL_TTL: Duration = Duration::from_secs(600);
/// How often waiting answers are looked at.
const WATCH_EVERY: Duration = Duration::from_secs(2);
/// How long a refused poll waits before asking again: the other consumer
/// may have stopped.
const REFUSED_BACKOFF: Duration = Duration::from_secs(60);
/// How many tasks a lookup reads.
const LIST_LIMIT: u32 = 1000;

/// One gateway per channel that declares `inbound:`.
pub struct TelegramInterface {
    channel: String,
}

impl TelegramInterface {
    pub fn new(channel: impl Into<String>) -> Self {
        Self { channel: channel.into() }
    }
}

#[async_trait]
impl Interface<Engine> for TelegramInterface {
    fn name(&self) -> &str {
        "telegram"
    }

    fn description(&self) -> String {
        format!("Telegram, through the channel {}", self.channel)
    }

    async fn serve(
        self: Arc<Self>,
        engine: Arc<Engine>,
        _ctx: InterfaceContext,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        let snapshot = engine.factory_snapshot();
        let inbound = ch::find(&snapshot.config.channels, &self.channel)
            .and_then(|c| c.inbound.clone())
            .ok_or_else(|| FactoryError::BadRequest(format!("the channel {} has no inbound: block", self.channel)))?;
        // A message must have somewhere to go: refused now, in front of
        // whoever started the daemon, rather than on the first message.
        snapshot.scope(&inbound.scope)?;
        engine.resolve_agent(&inbound.scope, &inbound.agent)?;
        // The daemon sends blocked runs' questions to this channel itself
        // (`daemon.escalation`), each one answerable by a reply: the
        // gateway's own "waiting for you" would only say it twice.
        let escalates = snapshot.config.daemon.escalation.channel.as_deref() == Some(self.channel.as_str());
        let gateway = Arc::new(Gateway {
            engine,
            escalates,
            channel: self.channel.clone(),
            consumer: format!("telegram-gateway-{}", uuid::Uuid::new_v4()),
            words: Words(inbound.language.unwrap_or_default()),
            inbound,
            state: Mutex::new(State::default()),
        });
        gateway.run(shutdown).await;
        Ok(())
    }
}

#[derive(Default)]
struct State {
    /// Questions waiting for a yes, oldest first.
    proposals: Vec<Proposal>,
    /// Tasks a message became, by id, and the message each answers.
    awaiting: HashMap<String, Awaiting>,
}

struct Awaiting {
    message: i64,
    /// When the block already passed on was reported, so each is said once.
    told_blocked: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
enum Action {
    Run { task: String, title: String },
    Schedule { task: String, title: String, schedule: Schedule, next: DateTime<Utc> },
}

#[derive(Debug, Clone)]
struct Proposal {
    id: String,
    action: Action,
    /// The message the question answers.
    message: i64,
    at: Instant,
}

/// What a message asks for, when it is one of the gateway's own commands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Help,
    Jobs,
    Run(String),
    Schedule(String),
    Intake(String),
    Unknown(String),
}

/// `/jobs`, `/run@my_bot x`, ... or `None` for anything that is not a
/// command and goes to the assistant.
fn command(text: &str) -> Option<Command> {
    let text = text.trim();
    let rest = text.strip_prefix('/')?;
    let (head, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let name = head.split('@').next().unwrap_or_default().to_ascii_lowercase();
    let arg = arg.trim().to_string();
    Some(match name.as_str() {
        "start" | "help" | "hilfe" => Command::Help,
        "jobs" | "list" | "tasks" => Command::Jobs,
        "run" => Command::Run(arg),
        "schedule" | "reschedule" => Command::Schedule(arg),
        "intake" => Command::Intake(arg),
        _ => Command::Unknown(head.to_string()),
    })
}

/// A yes or a no to a waiting question, in the owner's words.
fn yes_or_no(text: &str) -> Option<bool> {
    let word = text.trim().trim_end_matches(['!', '.', ' ']).to_lowercase();
    match word.as_str() {
        "ja" | "j" | "jo" | "jep" | "yes" | "y" | "ok" | "okay" => Some(true),
        "nein" | "n" | "no" | "nö" | "abbrechen" | "cancel" | "stop" => Some(false),
        _ => None,
    }
}

/// `<task> <minute> <hour> <day> <month> <weekday> [timezone]`: the task as
/// it was named, the cron expression, and the timezone if one was given.
/// The task may be a title with spaces; the cron is always the five fields
/// before the end or before a timezone (a name with a `/`, or `UTC`).
fn schedule_args(arg: &str) -> std::result::Result<(String, String, Option<String>), String> {
    let mut words: Vec<&str> = arg.split_whitespace().collect();
    let timezone = match words.last() {
        Some(last) if last.contains('/') || last.eq_ignore_ascii_case("utc") => words.pop().map(str::to_string),
        _ => None,
    };
    if words.len() < 6 {
        return Err("name the task, then the five cron fields: /schedule <task> <minute> <hour> <day> <month> <weekday> [timezone]".into());
    }
    let cron = words.split_off(words.len() - 5).join(" ");
    Ok((words.join(" "), cron, timezone))
}

/// The one task `name` means: its id, a unique id prefix of four or more
/// characters, its title (any case), or a unique part of a title. Tasks a
/// message became are not candidates.
fn match_task<'a>(tasks: &'a [Task], name: &str) -> std::result::Result<&'a Task, Vec<&'a Task>> {
    let name = name.trim();
    let wanted = name.to_lowercase();
    let candidates: Vec<&Task> = tasks.iter().filter(|t| !t.labels.contains_key(ch::LABEL_CHANNEL)).collect();
    let pick = |found: Vec<&'a Task>| match found.as_slice() {
        [one] => Some(Ok(*one)),
        [] => None,
        _ => Some(Err(found)),
    };
    if name.is_empty() {
        return Err(Vec::new());
    }
    if let Some(task) = candidates.iter().find(|t| t.id == name) {
        return Ok(task);
    }
    if name.len() >= 4 {
        if let Some(found) = pick(candidates.iter().copied().filter(|t| t.id.starts_with(name)).collect()) {
            return found;
        }
    }
    if let Some(found) = pick(candidates.iter().copied().filter(|t| t.title.to_lowercase() == wanted).collect()) {
        return found;
    }
    pick(candidates.iter().copied().filter(|t| t.title.to_lowercase().contains(&wanted)).collect()).unwrap_or(Err(Vec::new()))
}

/// The last line of an answer, when it is a command the assistant proposes
/// rather than says: `/run x`, `/schedule x ...` or `/jobs`. The answer
/// without it, and the command.
fn proposal_line(answer: &str) -> (String, Option<Command>) {
    let trimmed = answer.trim_end();
    let (before, last) = match trimmed.rsplit_once('\n') {
        Some((before, last)) => (before, last),
        None => ("", trimmed),
    };
    match command(last.trim().trim_matches('`')) {
        Some(found @ (Command::Run(_) | Command::Schedule(_) | Command::Jobs)) => (before.trim_end().to_string(), Some(found)),
        _ => (answer.to_string(), None),
    }
}

/// The instructions a message becomes: its text, and the files it brought
/// as a list a person, a model and a shell all read the same way -- a
/// shell as comments.
fn instructions(text: &str, files: &[ReceivedFile], refused: &[String]) -> String {
    let mut out = text.trim().to_string();
    if !files.is_empty() {
        out.push_str("\n\n# Files sent with this message, saved on this host:");
        for file in files {
            out.push_str(&format!("\n#   {} ({}, {} bytes)", file.path, file.kind, file.bytes));
        }
    }
    if !refused.is_empty() {
        out.push_str("\n\n# Files sent with this message that were not taken:");
        for why in refused {
            out.push_str(&format!("\n#   {why}"));
        }
    }
    out
}

/// A task's title from a message: its first line, cut short.
fn title(text: &str, files: &[ReceivedFile]) -> String {
    let first = text.lines().map(str::trim).find(|l| !l.is_empty());
    let line = match (first, files.first()) {
        (Some(line), _) => line.to_string(),
        (None, Some(file)) => format!("a {}", file.kind.replace('_', " ")),
        (None, None) => "a message".into(),
    };
    let short: String = line.chars().take(60).collect();
    format!("Telegram: {}{}", short, if line.chars().count() > 60 { "…" } else { "" })
}

fn short_id(id: &str) -> &str {
    &id[..id.len().min(8)]
}

/// When `next` falls, on the schedule's own clock.
fn local_time(next: DateTime<Utc>, schedule: Option<&Schedule>) -> String {
    match schedule {
        Some(Schedule::Cron(CronSchedule { timezone: Some(tz), .. })) => match crate::schedule::timezone(tz) {
            Ok(zone) => format!("{} {tz}", next.with_timezone(&zone).format("%a %Y-%m-%d %H:%M")),
            Err(_) => format!("{} UTC", next.format("%a %Y-%m-%d %H:%M")),
        },
        _ => format!("{} UTC", next.format("%a %Y-%m-%d %H:%M")),
    }
}

/// The gateway's own lines, in the configured language.
struct Words(Language);

impl Words {
    fn de(&self) -> bool {
        self.0 == Language::De
    }
    fn pick(&self, en: &str, de: &str) -> String {
        if self.de() { de } else { en }.to_string()
    }
    fn ack(&self) -> String {
        self.pick("On it. I'll answer here when it is done.", "Bin dran – ich antworte hier, sobald es fertig ist.")
    }
    fn help(&self) -> String {
        self.pick(
            "Write to me as you would to the assistant; documents, photos and voice notes are passed on.\n\n\
             /jobs – scheduled and running tasks\n\
             /run <task> – run a task now\n\
             /schedule <task> <minute> <hour> <day> <month> <weekday> [timezone] – reschedule it, after you confirm\n\
             /intake <text> – hand a request to Factory's intake",
            "Schreib mir wie dem Assistenten; Dokumente, Fotos und Sprachnachrichten werden weitergegeben.\n\n\
             /jobs – geplante und laufende Aufgaben\n\
             /run <Aufgabe> – eine Aufgabe jetzt starten\n\
             /schedule <Aufgabe> <Minute> <Stunde> <Tag> <Monat> <Wochentag> [Zeitzone] – neu planen, nach deiner Bestätigung\n\
             /intake <Text> – eine Anfrage an Factorys Intake geben",
        )
    }
    fn yes(&self) -> String {
        self.pick("Yes", "Ja")
    }
    fn no(&self) -> String {
        self.pick("No", "Nein")
    }
    fn question(&self, action: &Action) -> String {
        match action {
            Action::Run { title, task } => {
                if self.de() {
                    format!("„{title}“ ({}) jetzt starten?", short_id(task))
                } else {
                    format!("Run “{title}” ({}) now?", short_id(task))
                }
            }
            Action::Schedule { title, task, schedule, next } => {
                let when = local_time(*next, Some(schedule));
                let plan = crate::operations::describe_schedule(Some(schedule));
                if self.de() {
                    format!("„{title}“ ({}) auf {plan} umplanen? Nächster Lauf: {when}.", short_id(task))
                } else {
                    format!("Reschedule “{title}” ({}) to {plan}? Next run: {when}.", short_id(task))
                }
            }
        }
    }
    fn expired(&self) -> String {
        self.pick("That question has expired or was already answered; ask again.", "Diese Frage ist abgelaufen oder schon beantwortet; frag bitte neu.")
    }
    fn declined(&self) -> String {
        self.pick("Left as it is.", "Bleibt, wie es ist.")
    }
    fn started(&self, title: &str) -> String {
        if self.de() {
            format!("Gestartet: {title}. Das Ergebnis steht in Factory.")
        } else {
            format!("Started: {title}. Its result is in Factory.")
        }
    }
    fn rescheduled(&self, title: &str, plan: &str, when: &str) -> String {
        if self.de() {
            format!("Umgeplant: {title} läuft jetzt {plan}. Nächster Lauf: {when}.")
        } else {
            format!("Rescheduled: {title} now runs {plan}. Next run: {when}.")
        }
    }
    fn failed(&self, what: &str) -> String {
        if self.de() {
            format!("Das ging nicht: {what}")
        } else {
            format!("That did not work: {what}")
        }
    }
    fn not_found(&self, name: &str, candidates: &[&Task]) -> String {
        let mut out = if candidates.is_empty() {
            if self.de() {
                format!("Keine Aufgabe passt zu „{name}“. /jobs zeigt die geplanten.")
            } else {
                format!("No task matches “{name}”. /jobs lists the scheduled ones.")
            }
        } else if self.de() {
            format!("„{name}“ passt auf mehrere Aufgaben:")
        } else {
            format!("“{name}” matches more than one task:")
        };
        for task in candidates.iter().take(8) {
            out.push_str(&format!("\n• {} ({})", task.title, short_id(&task.id)));
        }
        out
    }
    fn unknown(&self, name: &str) -> String {
        if self.de() {
            format!("/{name} kenne ich nicht.\n\n{}", self.help())
        } else {
            format!("I don't know /{name}.\n\n{}", self.help())
        }
    }
    fn nothing(&self) -> String {
        self.pick(
            "That message had nothing I can pass on: send text, a document, a photo or a voice note.",
            "Diese Nachricht hatte nichts, was ich weitergeben kann: schick Text, ein Dokument, ein Foto oder eine Sprachnachricht.",
        )
    }
    fn not_taken(&self, refused: &[String]) -> String {
        let head = self.pick("Not taken:", "Nicht übernommen:");
        format!("{head}\n{}", refused.iter().map(|r| format!("• {r}")).collect::<Vec<_>>().join("\n"))
    }
    fn handed_in(&self, id: &str) -> String {
        if self.de() {
            format!("An Factorys Intake übergeben ({}); dort wird sie geprüft.", short_id(id))
        } else {
            format!("Handed to Factory's intake ({}) for triage.", short_id(id))
        }
    }
    fn answer(&self, task: &Task, blocked: Option<&str>) -> String {
        match (task.status, blocked) {
            (TaskStatus::Blocked, Some(why)) => {
                if self.de() {
                    format!("Wartet auf dich: {why}\n(Antworten in Factory: Aufgabe {})", short_id(&task.id))
                } else {
                    format!("Waiting for you: {why}\n(Answer it in Factory: task {})", short_id(&task.id))
                }
            }
            (TaskStatus::Done, _) => task
                .result
                .clone()
                .filter(|r| !r.trim().is_empty())
                .unwrap_or_else(|| self.pick("Done.", "Erledigt.")),
            (TaskStatus::Cancelled, _) => self.pick("Cancelled.", "Abgebrochen."),
            _ => self.failed(task.error.as_deref().unwrap_or("the run ended without a result")),
        }
    }
    fn jobs(&self, tasks: &[Task]) -> String {
        let mut scheduled: Vec<&Task> = tasks.iter().filter(|t| t.schedule.is_some() && t.status != TaskStatus::Intake).collect();
        scheduled.sort_by_key(|t| (t.schedule_paused, t.next_run_at));
        let running: Vec<&Task> = tasks
            .iter()
            .filter(|t| matches!(t.status, TaskStatus::Dispatching | TaskStatus::Running | TaskStatus::Blocked | TaskStatus::Verifying))
            .collect();
        let mut out = String::new();
        out.push_str(&self.pick("Running now:", "Läuft gerade:"));
        if running.is_empty() {
            out.push_str(&self.pick("\n• nothing", "\n• nichts"));
        }
        for task in running.iter().take(20) {
            out.push_str(&format!("\n• {} ({}, {})", task.title, task.status.as_str(), short_id(&task.id)));
        }
        out.push_str(&self.pick("\n\nScheduled:", "\n\nGeplant:"));
        if scheduled.is_empty() {
            out.push_str(&self.pick("\n• nothing", "\n• nichts"));
        }
        for task in scheduled.iter().take(30) {
            let plan = crate::operations::describe_schedule(task.schedule.as_ref());
            let when = match (task.schedule_paused, task.next_run_at) {
                (true, _) => self.pick("paused", "pausiert"),
                (false, Some(next)) => format!("{} {}", self.pick("next", "nächster"), local_time(next, task.schedule.as_ref())),
                (false, None) => String::new(),
            };
            out.push_str(&format!("\n• {} – {plan}, {when} ({})", task.title, short_id(&task.id)));
        }
        out
    }
}

struct Gateway {
    engine: Arc<Engine>,
    /// The daemon sends this channel the questions blocked runs ask.
    escalates: bool,
    channel: String,
    consumer: String,
    inbound: InboundDecl,
    words: Words,
    state: Mutex<State>,
}

/// A refused request: its code and the daemon's own words.
type Refused = (String, String);

impl Gateway {
    /// One request, as the owner, through the one handler.
    async fn ask(&self, request: Request) -> std::result::Result<Payload, Refused> {
        match Box::pin(self.engine.handle(Envelope { request, token: None })).await {
            Response::Ok { data } => Ok(data),
            Response::Error { code, message } => Err((code, message)),
        }
    }

    async fn record(&self, kind: InboundKind, message: String, data: serde_json::Value) {
        let record = InboundRecord { kind, message, data };
        if let Err((_, why)) = self.ask(Request::ChannelRecord { channel: self.channel.clone(), record }).await {
            tracing::warn!(channel = %self.channel, "could not journal an inbound record: {why}");
        }
    }

    /// Send `text` to the owner, as an answer to `message`. A text too long
    /// for ten Telegram messages goes as a file instead.
    async fn say(&self, message: Option<i64>, text: String, buttons: Vec<Button>) {
        let (text, files, _staged) = match ch::split_text(&text) {
            Err(_) if !text.trim().is_empty() => match self.stage(&text) {
                Ok(staged) => (
                    self.words.pick("The answer is long, so it comes as a file.", "Die Antwort ist lang, deshalb kommt sie als Datei."),
                    vec![staged.path().display().to_string()],
                    Some(staged),
                ),
                Err(why) => (self.words.failed(&why), Vec::new(), None),
            },
            _ => (text, Vec::new(), None),
        };
        let request = Request::Notify { channel: self.channel.clone(), text, files, reply_to: message, buttons };
        if let Err((_, why)) = self.ask(request).await {
            tracing::warn!(channel = %self.channel, "could not answer on the channel: {why}");
        }
    }

    /// A long answer written to a private file to be sent as a document.
    fn stage(&self, text: &str) -> std::result::Result<Staged, String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("factory-telegram-{}", uuid::Uuid::new_v4()));
        {
            use std::os::unix::fs::DirBuilderExt;
            std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| format!("could not stage the answer: {e}"))?;
        }
        let staged = Staged(dir);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(staged.path())
            .map_err(|e| format!("could not stage the answer: {e}"))?;
        file.write_all(text.as_bytes()).map_err(|e| format!("could not stage the answer: {e}"))?;
        Ok(staged)
    }

    async fn run(self: Arc<Self>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        self.record(
            InboundKind::Started,
            format!("taking messages in, as tasks for {} in scope {}", self.inbound.agent, self.inbound.scope),
            serde_json::json!({ "scope": self.inbound.scope, "agent": self.inbound.agent }),
        )
        .await;
        self.recover().await;
        let watcher = {
            let gateway = self.clone();
            let mut shutdown = shutdown.clone();
            tokio::spawn(async move {
                let mut tick = tokio::time::interval(WATCH_EVERY);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tokio::select! {
                        _ = tick.tick() => gateway.watch().await,
                        changed = shutdown.changed() => if changed.is_err() || *shutdown.borrow() { return },
                    }
                }
            })
        };
        let (mut offset, mut said_refused, mut backoff): (Option<i64>, Option<String>, Duration) = (None, None, Duration::from_secs(1));
        loop {
            let poll = self.ask(Request::ChannelPoll {
                channel: self.channel.clone(),
                consumer: self.consumer.clone(),
                offset,
                wait: None,
            });
            let answer = tokio::select! {
                answer = poll => answer,
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break }
                    continue;
                }
            };
            let pause = match answer {
                Ok(Payload::ChannelUpdates { batch }) => {
                    said_refused = None;
                    backoff = Duration::from_secs(1);
                    for item in batch.items {
                        self.take(item).await;
                    }
                    offset = batch.next_offset.or(offset);
                    None
                }
                Ok(other) => {
                    tracing::warn!(channel = %self.channel, "channel.poll answered {other:?}");
                    Some(REFUSED_BACKOFF)
                }
                // Another consumer holds the token, or Telegram says one
                // does: said once, then asked again a minute later.
                Err((code, why)) if code == "denied" => {
                    if said_refused.as_deref() != Some(why.as_str()) {
                        tracing::warn!(channel = %self.channel, "{why}");
                        self.record(InboundKind::Refused, why.clone(), serde_json::json!({})).await;
                        said_refused = Some(why);
                    }
                    Some(REFUSED_BACKOFF)
                }
                Err((_, why)) => {
                    tracing::warn!(channel = %self.channel, "polling failed, trying again in {}s: {why}", backoff.as_secs());
                    let wait = backoff;
                    backoff = (backoff * 2).min(Duration::from_secs(60));
                    Some(wait)
                }
            };
            if let Some(pause) = pause {
                tokio::select! {
                    _ = tokio::time::sleep(pause) => {}
                    changed = shutdown.changed() => if changed.is_err() || *shutdown.borrow() { break },
                }
            }
        }
        watcher.abort();
    }

    async fn take(&self, item: ch::Inbound) {
        match item {
            ch::Inbound::Ignored { update, from, chat, what } => {
                self.record(InboundKind::Ignored, format!("ignored {what}"), serde_json::json!({ "update": update, "user": from, "chat": chat }))
                    .await;
            }
            ch::Inbound::Button { message, data, .. } => {
                let (verdict, id) = data.split_once(':').unwrap_or(("", ""));
                match verdict {
                    "ok" => self.decide(id, true, message).await,
                    "no" => self.decide(id, false, message).await,
                    crate::escalation::ANSWER_BUTTON => self.choose(id, message).await,
                    _ => self.say(message, self.words.expired(), Vec::new()).await,
                }
            }
            ch::Inbound::Message { update, message, from, at, text, files, refused, reply_to } => {
                // A reply to a question Factory sent answers it; a reply to
                // anything else is a message like any other.
                if let Some(to) = reply_to.filter(|_| !text.trim().is_empty()) {
                    if self.answer_reply(to, &text, message).await {
                        return;
                    }
                }
                if files.is_empty() {
                    if let Some(yes) = yes_or_no(&text) {
                        let newest = self.state.lock().unwrap().proposals.last().map(|p| p.id.clone());
                        if let Some(id) = newest {
                            return self.decide(&id, yes, Some(message)).await;
                        }
                    }
                }
                match command(&text) {
                    Some(Command::Help) => self.say(Some(message), self.words.help(), Vec::new()).await,
                    Some(Command::Jobs) => self.jobs(message).await,
                    Some(Command::Run(name)) => self.propose_run(message, &name, false).await,
                    Some(Command::Schedule(arg)) => self.propose_schedule(message, &arg).await,
                    Some(Command::Intake(request)) => self.intake(update, message, from, at, &request).await,
                    Some(Command::Unknown(name)) => self.say(Some(message), self.words.unknown(&name), Vec::new()).await,
                    None if text.trim().is_empty() && files.is_empty() => {
                        let said = if refused.is_empty() { self.words.nothing() } else { self.words.not_taken(&refused) };
                        self.say(Some(message), said, Vec::new()).await;
                    }
                    None => self.converse(update, message, &text, &files, &refused).await,
                }
            }
        }
    }

    async fn tasks(&self, scope: Option<&str>) -> std::result::Result<Vec<Task>, String> {
        let filter = TaskFilter { scope: scope.map(str::to_string), limit: Some(LIST_LIMIT), ..Default::default() };
        match self.ask(Request::TaskList(filter)).await {
            Ok(Payload::Tasks { tasks }) => Ok(tasks),
            Ok(other) => Err(format!("task.list answered {other:?}")),
            Err((_, why)) => Err(why),
        }
    }

    async fn jobs(&self, message: i64) {
        let said = match self.tasks(None).await {
            Ok(tasks) => self.words.jobs(&tasks),
            Err(why) => self.words.failed(&why),
        };
        self.say(Some(message), said, Vec::new()).await;
    }

    /// The task `name` means, or the answer that says why there is none.
    async fn find(&self, name: &str) -> std::result::Result<Task, String> {
        let tasks = self.tasks(None).await.map_err(|why| self.words.failed(&why))?;
        match match_task(&tasks, name) {
            Ok(task) => Ok(task.clone()),
            Err(candidates) => Err(self.words.not_found(name, &candidates)),
        }
    }

    /// `/run <task>`: the owner said so, so it runs now. Proposed by the
    /// assistant, it is asked first.
    async fn propose_run(&self, message: i64, name: &str, ask_first: bool) {
        let task = match self.find(name).await {
            Ok(task) => task,
            Err(said) => return self.say(Some(message), said, Vec::new()).await,
        };
        let action = Action::Run { task: task.id.clone(), title: task.title.clone() };
        if ask_first {
            self.ask_owner(message, action, String::new()).await;
        } else {
            self.act(message, action, "asked on Telegram").await;
        }
    }

    /// `/schedule <task> <cron> [timezone]`: never done on the spot. A
    /// timezone left out keeps the task's own, rather than turning it into
    /// UTC the way a bare `--schedule` would.
    async fn propose_schedule(&self, message: i64, arg: &str) {
        let (name, expr, timezone) = match schedule_args(arg) {
            Ok(parts) => parts,
            Err(why) => return self.say(Some(message), self.words.failed(&why), Vec::new()).await,
        };
        let task = match self.find(&name).await {
            Ok(task) => task,
            Err(said) => return self.say(Some(message), said, Vec::new()).await,
        };
        let timezone = timezone.or_else(|| match &task.schedule {
            Some(Schedule::Cron(CronSchedule { timezone, .. })) => timezone.clone(),
            _ => None,
        });
        let schedule = Schedule::Cron(CronSchedule { expr, timezone });
        let next = match factory_kernel::schedule_grid::next_after(&schedule, Utc::now()) {
            Ok(next) => next,
            Err(why) => return self.say(Some(message), self.words.failed(&why.to_string()), Vec::new()).await,
        };
        let action = Action::Schedule { task: task.id.clone(), title: task.title.clone(), schedule, next };
        self.ask_owner(message, action, String::new()).await;
    }

    /// Ask the owner, with buttons, and wait for the yes.
    async fn ask_owner(&self, message: i64, action: Action, preface: String) {
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let question = self.words.question(&action);
        let what = match &action {
            Action::Run { task, .. } => serde_json::json!({ "action": "run", "task": task }),
            Action::Schedule { task, schedule, .. } => serde_json::json!({ "action": "schedule", "task": task, "schedule": schedule }),
        };
        {
            let mut state = self.state.lock().unwrap();
            state.proposals.retain(|p| p.at.elapsed() < PROPOSAL_TTL);
            if state.proposals.len() >= 20 {
                state.proposals.remove(0);
            }
            state.proposals.push(Proposal { id: id.clone(), action, message, at: Instant::now() });
        }
        self.record(InboundKind::Proposed, format!("asked the owner to confirm: {question}"), what).await;
        let buttons = vec![
            Button { text: self.words.yes(), data: format!("ok:{id}") },
            Button { text: self.words.no(), data: format!("no:{id}") },
        ];
        let text = if preface.trim().is_empty() { question } else { format!("{preface}\n\n{question}") };
        self.say(Some(message), text, buttons).await;
    }

    /// The owner's yes or no to question `id`.
    async fn decide(&self, id: &str, yes: bool, message: Option<i64>) {
        let proposal = {
            let mut state = self.state.lock().unwrap();
            state.proposals.retain(|p| p.at.elapsed() < PROPOSAL_TTL);
            state.proposals.iter().position(|p| p.id == id).map(|at| state.proposals.remove(at))
        };
        let Some(proposal) = proposal else {
            return self.say(message, self.words.expired(), Vec::new()).await;
        };
        let reply = message.or(Some(proposal.message));
        if !yes {
            self.record(InboundKind::Declined, format!("the owner declined: {}", self.words.question(&proposal.action)), serde_json::json!({}))
                .await;
            return self.say(reply, self.words.declined(), Vec::new()).await;
        }
        self.record(InboundKind::Confirmed, format!("the owner confirmed: {}", self.words.question(&proposal.action)), serde_json::json!({}))
            .await;
        self.act(reply.unwrap_or(proposal.message), proposal.action, "confirmed on Telegram").await;
    }

    /// Do it, through the one handler, as the owner.
    async fn act(&self, message: i64, action: Action, reason: &str) {
        let said = match action {
            Action::Run { task, title } => {
                let request = Request::TaskRun { id: task, reason: Some(reason.into()), continue_run: false, override_wait: false };
                match self.ask(request).await {
                    Ok(_) => self.words.started(&title),
                    Err((_, why)) => self.words.failed(&why),
                }
            }
            Action::Schedule { task, title, schedule, .. } => {
                let patch = TaskPatch { schedule: Some(schedule), ..Default::default() };
                match self.ask(Request::TaskUpdate { id: task, patch, reason: Some(reason.into()) }).await {
                    Ok(Payload::Task { task }) => {
                        let plan = crate::operations::describe_schedule(task.schedule.as_ref());
                        let when = task.next_run_at.map(|n| local_time(n, task.schedule.as_ref())).unwrap_or_default();
                        self.words.rescheduled(&title, &plan, &when)
                    }
                    Ok(_) => self.words.rescheduled(&title, "", ""),
                    Err((_, why)) => self.words.failed(&why),
                }
            }
        };
        self.say(Some(message), said, Vec::new()).await;
    }

    /// `/intake <text>`: a request for Factory, handed in the way the
    /// iMessage assistant hands one in -- `chat`, relayed, keyed on the
    /// message so a second delivery makes no second item.
    async fn intake(&self, update: i64, message: i64, from: i64, at: DateTime<Utc>, request: &str) {
        let request = request.trim();
        if request.is_empty() {
            return self.say(Some(message), self.words.failed("/intake <text>"), Vec::new()).await;
        }
        let new = NewIntake {
            title: request.lines().next().unwrap_or(request).chars().take(80).collect(),
            instructions: request.to_string(),
            source: Some(SourceKind::Chat),
            provider: Some("telegram".into()),
            reference: Some(format!("{}/{message}", self.channel)),
            requester: Some(format!("telegram user {from}")),
            received_at: Some(at.min(Utc::now())),
            ..Default::default()
        };
        let said = match self.ask(Request::IntakeAdd(new)).await {
            Ok(Payload::Task { task }) => self.words.handed_in(&task.id),
            Ok(_) => self.words.handed_in(""),
            Err((_, why)) => {
                self.record(InboundKind::Failed, format!("could not hand update {update} to intake: {why}"), serde_json::json!({ "update": update }))
                    .await;
                self.words.failed(&why)
            }
        };
        self.say(Some(message), said, Vec::new()).await;
    }

    /// A message for the assistant: a task, run now, acknowledged now, and
    /// answered when its run ends.
    async fn converse(&self, update: i64, message: i64, text: &str, files: &[ReceivedFile], refused: &[String]) {
        // Taken in before a restart, and not yet confirmed to Telegram:
        // already a task, not a second one.
        if let Ok(tasks) = self.tasks(Some(&self.inbound.scope)).await {
            let mine = |t: &&Task| t.labels.get(ch::LABEL_CHANNEL) == Some(&self.channel);
            if tasks.iter().filter(mine).any(|t| t.labels.get(ch::LABEL_UPDATE) == Some(&update.to_string())) {
                return;
            }
        }
        let labels = BTreeMap::from([
            (ch::LABEL_CHANNEL.to_string(), self.channel.clone()),
            (ch::LABEL_MESSAGE.to_string(), message.to_string()),
            (ch::LABEL_UPDATE.to_string(), update.to_string()),
        ]);
        let new = NewTask {
            title: title(text, files),
            instructions: instructions(text, files, refused),
            scope: Some(self.inbound.scope.clone()),
            agent: Some(self.inbound.agent.clone()),
            worktree: Some(false),
            deliver: Some(self.channel.clone()),
            labels,
            ..Default::default()
        };
        let task = match self.ask(Request::TaskCreate(new)).await {
            Ok(Payload::Task { task }) => task,
            Ok(other) => return self.say(Some(message), self.words.failed(&format!("task.create answered {other:?}")), Vec::new()).await,
            Err((_, why)) => {
                self.record(InboundKind::Failed, format!("update {update} could not become a task: {why}"), serde_json::json!({ "update": update }))
                    .await;
                return self.say(Some(message), self.words.failed(&why), Vec::new()).await;
            }
        };
        let run = Request::TaskRun {
            id: task.id.clone(),
            reason: Some(format!("a message on {}", self.channel)),
            continue_run: false,
            override_wait: false,
        };
        if let Err((_, why)) = self.ask(run).await {
            return self.say(Some(message), self.words.failed(&why), Vec::new()).await;
        }
        let mut ack = self.words.ack();
        if !refused.is_empty() {
            ack = format!("{ack}\n\n{}", self.words.not_taken(refused));
        }
        self.say(Some(message), ack, Vec::new()).await;
        // Watched only once the acknowledgement went, so a run that ends at
        // once is still answered after it, never before.
        self.state.lock().unwrap().awaiting.insert(task.id.clone(), Awaiting { message, told_blocked: None });
    }

    /// After a restart: every task a message on this channel became that
    /// was not answered yet, waiting again. One that ended while nobody was
    /// watching is answered on the next look; one older than a day is let go.
    async fn recover(&self) {
        let Ok(tasks) = self.tasks(Some(&self.inbound.scope)).await else { return };
        let day_ago = Utc::now() - chrono::Duration::days(1);
        let mut state = self.state.lock().unwrap();
        for task in tasks {
            if task.labels.get(ch::LABEL_CHANNEL) != Some(&self.channel) || task.labels.contains_key(ch::LABEL_ANSWERED) {
                continue;
            }
            let Some(message) = task.labels.get(ch::LABEL_MESSAGE).and_then(|m| m.parse().ok()) else { continue };
            if task.updated_at >= day_ago {
                state.awaiting.insert(task.id.clone(), Awaiting { message, told_blocked: None });
            }
        }
    }

    /// Answer the question `to` carried with `text`; whether it carried one.
    /// What the daemon did, or why it would not, goes back as a reply.
    async fn answer_reply(&self, to: i64, text: &str, message: i64) -> bool {
        let request = Request::EscalationReply { channel: self.channel.clone(), message: to, text: text.to_string() };
        let said = match self.ask(request).await {
            Ok(Payload::Text { text }) => format!("✓ {text}"),
            Err((code, _)) if code == "not_found" => return false,
            Err((_, why)) => why,
            Ok(other) => format!("{other:?}"),
        };
        self.say(Some(message), said, Vec::new()).await;
        true
    }

    /// A question's button: `<run id>:<option>`.
    async fn choose(&self, pressed: &str, message: Option<i64>) {
        let Some((run, option)) = pressed.rsplit_once(':').and_then(|(run, n)| Some((run, n.parse().ok()?))) else {
            return self.say(message, self.words.expired(), Vec::new()).await;
        };
        let request = Request::EscalationChoose { channel: self.channel.clone(), run: run.to_string(), option };
        let said = match self.ask(request).await {
            Ok(Payload::Text { text }) => format!("✓ {text}"),
            Err((_, why)) => why,
            Ok(other) => format!("{other:?}"),
        };
        self.say(message, said, Vec::new()).await;
    }

    /// Look at every waiting answer once.
    async fn watch(&self) {
        let waiting: Vec<(String, i64, Option<DateTime<Utc>>)> =
            self.state.lock().unwrap().awaiting.iter().map(|(id, a)| (id.clone(), a.message, a.told_blocked)).collect();
        for (id, message, told_blocked) in waiting {
            let task = match self.ask(Request::TaskGet { id: id.clone() }).await {
                Ok(Payload::Task { task }) => task,
                Err((code, _)) if code == "not_found" => {
                    self.state.lock().unwrap().awaiting.remove(&id);
                    continue;
                }
                _ => continue,
            };
            match task.status {
                // The daemon sent the question itself, as a reply here.
                TaskStatus::Blocked if self.escalates => {}
                TaskStatus::Blocked => {
                    let (at, why) = self.blocked_reason(&task.id).await;
                    if told_blocked.is_some() && told_blocked >= at {
                        continue;
                    }
                    self.say(Some(message), self.words.answer(&task, Some(&why)), Vec::new()).await;
                    if let Some(waiting) = self.state.lock().unwrap().awaiting.get_mut(&id) {
                        waiting.told_blocked = Some(at.unwrap_or_else(Utc::now));
                    }
                }
                TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled => self.answer(&task, message).await,
                _ => {}
            }
        }
    }

    /// What a blocked run said it needs, and when: its newest `blocked`
    /// report.
    async fn blocked_reason(&self, task: &str) -> (Option<DateTime<Utc>>, String) {
        let newest = match self.ask(Request::TaskEntries { id: task.to_string(), limit: Some(50), task_only: false }).await {
            Ok(Payload::Entries { entries }) => entries.into_iter().filter(|e| e.kind == "blocked").max_by_key(|e| e.at),
            _ => None,
        };
        match newest {
            Some(entry) => (Some(entry.at), entry.message),
            None => (None, "(no reason given)".into()),
        }
    }

    /// The run ended: its answer goes back, once, and the task says so.
    async fn answer(&self, task: &Task, message: i64) {
        if self.state.lock().unwrap().awaiting.remove(&task.id).is_none() {
            return;
        }
        let said = self.words.answer(task, None);
        let (said, proposed) = if task.status == TaskStatus::Done { proposal_line(&said) } else { (said, None) };
        // Marked first: a restart in between loses one answer rather than
        // sending it twice.
        let mut labels = task.labels.clone();
        labels.insert(ch::LABEL_ANSWERED.to_string(), task.runs.to_string());
        let patch = TaskPatch { labels: Some(labels), ..Default::default() };
        if let Err((_, why)) = self.ask(Request::TaskUpdate { id: task.id.clone(), patch, reason: None }).await {
            tracing::warn!(channel = %self.channel, task = %task.id, "could not mark the task answered: {why}");
        }
        match proposed {
            Some(Command::Run(name)) => {
                self.say(Some(message), said, Vec::new()).await;
                self.propose_run(message, &name, true).await;
            }
            Some(Command::Schedule(arg)) => {
                self.say(Some(message), said, Vec::new()).await;
                self.propose_schedule(message, &arg).await;
            }
            Some(Command::Jobs) => {
                self.say(Some(message), said, Vec::new()).await;
                self.jobs(message).await;
            }
            _ => self.say(Some(message), said, Vec::new()).await,
        }
    }
}

/// A staged answer file, removed when the send is over.
struct Staged(std::path::PathBuf);

impl Staged {
    fn path(&self) -> std::path::PathBuf {
        self.0.join("answer.md")
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
#[path = "telegram_tests.rs"]
mod engine_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, title: &str) -> Task {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": title, "instructions": "", "scope": "demo", "agent": "shell", "runtime": "herdr",
            "status": "pending", "runs": 0, "created_at": "2026-10-01T00:00:00Z", "updated_at": "2026-10-01T00:00:00Z",
        }))
        .unwrap()
    }

    #[test]
    fn commands_are_read_with_or_without_the_bots_name_and_anything_else_goes_to_the_assistant() {
        assert_eq!(command("/jobs"), Some(Command::Jobs));
        assert_eq!(command("/jobs@factory_bot"), Some(Command::Jobs));
        assert_eq!(command("/run Konferenz-Radar"), Some(Command::Run("Konferenz-Radar".into())));
        assert_eq!(command("/schedule radar 0 8 * * 5"), Some(Command::Schedule("radar 0 8 * * 5".into())));
        assert_eq!(command("/intake bitte prüfen"), Some(Command::Intake("bitte prüfen".into())));
        assert_eq!(command("/start"), Some(Command::Help));
        assert_eq!(command("/frobnicate"), Some(Command::Unknown("frobnicate".into())));
        assert_eq!(command("Lass den Konferenz-Radar jetzt laufen"), None);
        assert_eq!(command("echo /jobs"), None);
    }

    #[test]
    fn yes_and_no_are_the_owners_words_in_either_language() {
        for yes in ["Ja", "ja!", "yes", "OK", "j"] {
            assert_eq!(yes_or_no(yes), Some(true), "{yes}");
        }
        for no in ["Nein", "no.", "abbrechen"] {
            assert_eq!(yes_or_no(no), Some(false), "{no}");
        }
        assert_eq!(yes_or_no("ja, aber erst morgen"), None);
    }

    #[test]
    fn a_schedule_names_the_task_then_five_cron_fields_and_maybe_a_timezone() {
        assert_eq!(
            schedule_args("Konferenz-Radar 0 8 * * 5 Europe/Berlin").unwrap(),
            ("Konferenz-Radar".into(), "0 8 * * 5".into(), Some("Europe/Berlin".into()))
        );
        assert_eq!(schedule_args("weekly radar 0 8 * * 5").unwrap(), ("weekly radar".into(), "0 8 * * 5".into(), None));
        assert_eq!(schedule_args("abc 0 8 * * 5 utc").unwrap().2.as_deref(), Some("utc"));
        assert!(schedule_args("0 8 * * 5").unwrap_err().contains("five cron fields"));
    }

    #[test]
    fn a_task_is_found_by_id_prefix_or_title_and_never_among_the_messages() {
        let mut message = task("cccc0000-1", "Telegram: radar");
        message.labels.insert(ch::LABEL_CHANNEL.into(), "owner-telegram".into());
        let tasks = vec![task("aaaa1111-x", "Konferenz-Radar"), task("bbbb2222-y", "Weekly worktree cleanup"), message];
        assert_eq!(match_task(&tasks, "aaaa1111-x").unwrap().title, "Konferenz-Radar");
        assert_eq!(match_task(&tasks, "bbbb").unwrap().title, "Weekly worktree cleanup");
        assert_eq!(match_task(&tasks, "konferenz-radar").unwrap().id, "aaaa1111-x");
        assert_eq!(match_task(&tasks, "radar").unwrap().id, "aaaa1111-x", "the message task is not a candidate");
        assert!(match_task(&tasks, "nothing like it").unwrap_err().is_empty());
        let two = vec![task("1", "Radar A"), task("2", "Radar B")];
        assert_eq!(match_task(&two, "radar").unwrap_err().len(), 2);
    }

    #[test]
    fn a_proposal_is_the_last_line_of_an_answer_and_only_a_task_command() {
        assert_eq!(
            proposal_line("Mache ich.\n/run Konferenz-Radar"),
            ("Mache ich.".into(), Some(Command::Run("Konferenz-Radar".into())))
        );
        assert_eq!(proposal_line("`/schedule radar 0 8 * * 5 Europe/Berlin`").1, Some(Command::Schedule("radar 0 8 * * 5 Europe/Berlin".into())));
        assert_eq!(proposal_line("Nur eine Antwort.").1, None);
        assert_eq!(proposal_line("Siehe\n/intake das nicht").1, None);
    }

    #[test]
    fn instructions_list_the_files_as_comments_a_shell_skips() {
        let files = vec![ReceivedFile { path: "/x/inbox/1-0-a.pdf".into(), kind: "document".into(), bytes: 3, mime: None }];
        let text = instructions("cat /x/inbox/1-0-a.pdf", &files, &["big.bin (document): too big".into()]);
        assert!(text.starts_with("cat /x/inbox/1-0-a.pdf\n\n# Files sent"), "{text}");
        assert!(text.lines().skip(1).filter(|l| !l.is_empty()).all(|l| l.starts_with('#')), "{text}");
        assert_eq!(title("", &files), "Telegram: a document");
        assert!(title(&"x".repeat(80), &[]).ends_with('…'));
    }
}
