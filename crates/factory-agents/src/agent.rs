//! Standing agents: the ones that are not a task.
//!
//! A task agent exists for the length of one run. A permanent agent just runs,
//! idle or not, because somebody wants it there -- an assistant to talk to, a
//! watcher, a session kept warm. It is declared in the instance config and the
//! daemon keeps it alive; it is never failed for being quiet, because being
//! quiet is what it is for.

use chrono::{DateTime, Utc};
use factory_kernel::SessionRef;
use serde::{Deserialize, Serialize};

/// What an agent is allowed to do lives next door, in `role`. It is re-exported
/// here because an agent is where a role is worn.
pub use crate::role::Role;

/// How long an agent is meant to stick around.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifetime {
    /// Started with the daemon and restarted if its session dies.
    Permanent,
    /// The same thing, but only when somebody asks for it. Nothing starts or
    /// restarts it on its own.
    Temporary,
    /// Not a standing agent at all: offered for tasks in this scope.
    Task,
}

impl Default for Lifetime {
    fn default() -> Self {
        Self::Task
    }
}

impl Lifetime {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permanent => "permanent",
            Self::Temporary => "temporary",
            Self::Task => "task",
        }
    }

    /// A standing agent is one that exists between tasks.
    pub fn is_standing(self) -> bool {
        matches!(self, Self::Permanent | Self::Temporary)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// A session is being opened.
    Starting,
    /// It is up.
    Ready,
    /// Its session went away without anybody asking.
    Gone,
    /// Somebody stopped it, and nothing will start it again on its own.
    Stopped,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Ready => "ready",
            Self::Gone => "gone",
            Self::Stopped => "stopped",
        }
    }

    pub fn is_live(self) -> bool {
        matches!(self, Self::Starting | Self::Ready)
    }
}

/// A standing agent as the daemon currently knows it -- one of its instances,
/// to be exact (`#329`): a declaration with `instances: 3` has three of these.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSession {
    /// `<scope>/<name>` for instance 1, `<scope>/<name>#<n>` for the others
    /// (`instance_id`). Deterministic, so reconciling against the config after
    /// a restart is a lookup rather than a search.
    pub id: String,
    pub name: String,
    pub scope: String,
    /// Which of its declaration's standing instances this is, counted from 1.
    /// Defaulted on read and left out when it is 1: every row written before
    /// instances existed is instance 1, under the id it always had.
    #[serde(default = "first", skip_serializing_if = "is_first")]
    pub instance: u32,
    /// The agent adapter.
    pub agent: String,
    pub runtime: String,
    pub lifetime: Lifetime,
    /// The role this agent is working under. It comes from the config unless
    /// somebody gave it one, in which case `assigned_role` says so and this
    /// mirrors it -- everything that asks what an agent may do reads this.
    ///
    /// Defaulted on read: a row written before roles existed is a worker,
    /// which is the same answer default-closed gives anyway.
    #[serde(default)]
    pub role: Role,
    /// A role a person gave this agent, which outlives a restart and wins over
    /// the config until it is cleared. Absent means the config decides -- and
    /// the roster shows the difference, so nobody has to wonder why the YAML
    /// in front of them says something else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_role: Option<Role>,
    pub state: AgentState,
    /// What this agent presents to say which agent it is. Never leaves the
    /// daemon: `redacted` strips it, like a run's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionRef>,
    /// The command a person types to get into this agent's terminal, when the
    /// runtime can name one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach: Option<String>,
    /// False once the config stops declaring it: still here, no longer wanted.
    #[serde(default = "yes")]
    pub declared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Who stopped it, while it stays stopped (`#330`): `"the owner"`, or the
    /// agent that asked. Cleared when it comes up again. What it says is what
    /// the roster shows next to `stopped`; the journal keeps the history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
}

fn yes() -> bool {
    true
}

fn first() -> u32 {
    1
}

fn is_first(instance: &u32) -> bool {
    *instance == 1
}

impl AgentSession {
    /// The declaration's own id, which is also its first instance's.
    pub fn id_for(scope: &str, name: &str) -> String {
        format!("{scope}/{name}")
    }

    /// One instance's id. Instance 1 keeps the id a standing agent had before
    /// it could have more than one, so its row, its guide and its attach line
    /// are exactly what they were; the rest are `<scope>/<name>#<n>`.
    pub fn instance_id(scope: &str, name: &str, instance: u32) -> String {
        match instance {
            0 | 1 => Self::id_for(scope, name),
            n => format!("{scope}/{name}#{n}"),
        }
    }

    /// The id as it is kept (`#330`): `scope/name#1` is `scope/name`, the id
    /// instance 1 has always had, so a person who counts from one finds it.
    /// Anything else is returned as given.
    pub fn canonical_id(id: &str) -> String {
        match id.strip_suffix("#1") {
            Some(bare) if !bare.ends_with('#') => bare.to_string(),
            _ => id.to_string(),
        }
    }

    /// An instance id taken apart: `(scope, name, instance)`. The name is
    /// what follows the last `/` (a name may contain neither `/` nor `#`),
    /// and a trailing `#<n>` is its instance; a bare `scope/name` is instance
    /// 1. `None` for something that is not an instance id at all.
    pub fn split_id(id: &str) -> Option<(String, String, u32)> {
        let (scope, rest) = id.rsplit_once('/')?;
        let (name, instance) = match rest.rsplit_once('#') {
            Some((name, n)) => (name, n.parse::<u32>().ok().filter(|n| *n >= 1)?),
            None => (rest, 1),
        };
        (!scope.is_empty() && !name.is_empty()).then(|| (scope.to_string(), name.to_string(), instance))
    }

    /// The declaration an instance id belongs to: the id without its `#<n>`.
    /// Two instances of one declaration are the same agent (`#329`), so this
    /// is what "itself" means to a check on its own reach.
    pub fn declaration_id(id: &str) -> &str {
        match id.rsplit_once('#') {
            Some((declaration, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => declaration,
            _ => id,
        }
    }

    /// What a person types to get into one instance's terminal from a shell
    /// (`#330`): `factory agent attach <id>`, the id quoted when a shell
    /// would otherwise read part of it -- every `#n` id, and a name with a
    /// space in it. Built here, once, so the CLI and the page print one
    /// spelling and never assemble an id themselves.
    pub fn connect_command(id: &str) -> String {
        format!("factory agent attach {}", Self::quoted_id(id))
    }

    /// An id as one shell word: as it is when nothing in it needs quoting,
    /// single-quoted otherwise.
    pub fn quoted_id(id: &str) -> String {
        let plain = id.bytes().all(|b| b.is_ascii_alphanumeric() || b"/._-".contains(&b));
        if plain {
            id.to_string()
        } else {
            format!("'{}'", id.replace('\'', "'\\''"))
        }
    }

    /// The id this agent would have been given before a scope's identity
    /// became its path -- keyed on just the last segment. A session persisted
    /// under the old, shorter scheme is found this way instead of being
    /// treated as gone and started a second time next to the one already
    /// running. `None` when there is nothing shorter to try: the root scope's
    /// name never changes, and a scope whose canonical name was already this
    /// short has no legacy form to distinguish it from the current one.
    pub fn legacy_id_for(scope: &str, name: &str) -> Option<String> {
        let last = scope.rsplit('/').next().unwrap_or(scope);
        if last == scope {
            return None;
        }
        Some(Self::id_for(last, name))
    }

    /// The role this agent works under, given what the config declares for it.
    /// One rule, in one place: an assignment wins until it is cleared, and the
    /// config decides for everything else.
    pub fn role_with(&self, declared: &Role) -> Role {
        self.assigned_role
            .clone()
            .unwrap_or_else(|| declared.clone())
    }

    pub fn redacted(&self) -> AgentSession {
        let mut a = self.clone();
        a.token = None;
        a
    }

    pub fn new(
        scope: &str,
        name: &str,
        agent: &str,
        runtime: &str,
        lifetime: Lifetime,
        role: Role,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Self::id_for(scope, name),
            name: name.to_string(),
            scope: scope.to_string(),
            instance: 1,
            agent: agent.to_string(),
            runtime: runtime.to_string(),
            lifetime,
            role,
            assigned_role: None,
            state: AgentState::Stopped,
            token: None,
            session: None,
            attach: None,
            stopped_by: None,
            stopped_at: None,
            declared: true,
            error: None,
            started_at: now,
            last_seen_at: now,
        }
    }

    /// The same row, as instance `instance` of its declaration.
    pub fn as_instance(mut self, instance: u32) -> Self {
        self.instance = instance.max(1);
        self.id = Self::instance_id(&self.scope, &self.name, self.instance);
        self
    }
}

/// Why a standing instance a scale-down would have stopped is still up. L3's
/// own observation only: a run belongs to L4 and is never looked at here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeptUp {
    /// Its runtime says it is in the middle of a turn.
    Working,
    /// Its runtime says it is waiting on an answer -- a question a person
    /// would lose if the session went away under it.
    Blocked,
    /// A person has its terminal open in Factory or typed at it a moment ago.
    Attached,
}

impl KeptUp {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Attached => "attached",
        }
    }

    pub fn words(self) -> &'static str {
        match self {
            Self::Working => "it is working",
            Self::Blocked => "it is waiting on an answer",
            Self::Attached => "a person is attached to it",
        }
    }
}

/// One instance a scale-down left up, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptInstance {
    pub id: String,
    pub instance: u32,
    pub why: KeptUp,
}

/// What a scale did (`#329`): the count asked for, what came up, what went
/// away, and what was left up because stopping it would have cost somebody
/// something. Instances left up are stopped once they are idle -- a scale
/// never forces one down.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaleReport {
    pub scope: String,
    pub name: String,
    /// The count the declaration said before this scale.
    pub previous: u32,
    /// The count it says now.
    pub wanted: u32,
    /// Instances live once this answered, the ones still draining included.
    pub up: u32,
    /// Instances being brought up, by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub started: Vec<String>,
    /// Instances stopped and removed, by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stopped: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<KeptInstance>,
}

impl ScaleReport {
    /// `"business-factory-claude in root: 3 wanted, 3 up"`, then what moved.
    pub fn summary(&self) -> String {
        let mut out = format!(
            "{} in {}: {} wanted, {} up",
            self.name, self.scope, self.wanted, self.up
        );
        if !self.started.is_empty() {
            out.push_str(&format!("; starting {}", self.started.join(", ")));
        }
        if !self.stopped.is_empty() {
            out.push_str(&format!("; stopped {}", self.stopped.join(", ")));
        }
        for kept in &self.kept {
            out.push_str(&format!(
                "; {} left up because {} -- it stops once it is idle",
                kept.id,
                kept.why.words()
            ));
        }
        out
    }
}

/// One thing stopping an instance now would cost somebody, in words that name
/// it: `"its terminal was open in Factory 4s ago"`, `"it is in the middle of a
/// turn"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopReason {
    pub why: KeptUp,
    pub detail: String,
}

/// What stopping one standing instance would cost right now (`#330`), asked
/// before a person stops it: the same three things a scale-down never stops
/// an instance for (`KeptUp`). L3's own observation, as `KeptUp` is -- a run
/// is L4's, and no run is ever given a standing agent's session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopCheck {
    pub id: String,
    pub instance: u32,
    /// Whether it is up at all. Stopping one that is not costs nothing.
    pub live: bool,
    /// Empty when nobody would lose anything; a person attached comes first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<StopReason>,
}

impl StopCheck {
    /// `"demo/watcher#2: its terminal was open in Factory 4s ago; it is in the
    /// middle of a turn"`, or `None` when stopping it costs nothing.
    pub fn warning(&self) -> Option<String> {
        (!self.reasons.is_empty()).then(|| {
            let reasons: Vec<&str> = self.reasons.iter().map(|r| r.detail.as_str()).collect();
            format!("{}: {}", self.id, reasons.join("; "))
        })
    }
}

/// Where `factory agent attach <id>` takes a person (`#330`): one instance's
/// session, resolved by the daemon from the runtime's own attach command --
/// never a herdr name guessed by whoever asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachTarget {
    pub id: String,
    pub scope: String,
    pub name: String,
    pub instance: u32,
    /// The runtime's command line, as a person would paste it.
    pub command: String,
    /// The same command, one word per argument: what the CLI executes.
    pub argv: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_redaction_and_assigned_role_precedence_remain_local_to_l3() {
        let mut agent = AgentSession::new(
            "projects/demo",
            "watcher",
            "shell",
            "runtime",
            Lifetime::Permanent,
            Role::worker(),
        );
        agent.token = Some("private-test-token".into());
        agent.assigned_role = Some(Role::new("limited"));
        let redacted = agent.redacted();
        assert!(redacted.token.is_none());
        assert!(agent.token.is_some());
        assert_eq!(redacted.id, "projects/demo/watcher");
        assert_eq!(redacted.role_with(&Role::foreman()).as_str(), "limited");
        agent.assigned_role = None;
        assert_eq!(agent.role_with(&Role::foreman()), Role::foreman());
        assert_eq!(
            AgentSession::legacy_id_for("projects/demo", "watcher"),
            Some("demo/watcher".into())
        );
        assert_eq!(AgentSession::legacy_id_for("demo", "watcher"), None);
    }

    #[test]
    fn instance_one_keeps_the_declarations_id_and_the_rest_carry_their_number() {
        assert_eq!(AgentSession::instance_id("projects/demo", "watcher", 1), "projects/demo/watcher");
        assert_eq!(AgentSession::instance_id("projects/demo", "watcher", 3), "projects/demo/watcher#3");
        let second = AgentSession::new("demo", "watcher", "shell", "runtime", Lifetime::Permanent, Role::worker())
            .as_instance(2);
        assert_eq!((second.id.as_str(), second.instance), ("demo/watcher#2", 2));
        // Instance 1 is written exactly as a row was before instances existed.
        let first = AgentSession::new("demo", "watcher", "shell", "runtime", Lifetime::Permanent, Role::worker());
        let json = serde_json::to_value(&first).unwrap();
        assert!(json.get("instance").is_none(), "{json}");
        assert_eq!(serde_json::to_value(&second).unwrap()["instance"], 2);
    }

    #[test]
    fn an_instance_id_names_its_declaration_and_its_number_and_one_is_the_bare_id() {
        assert_eq!(AgentSession::canonical_id("demo/watcher#1"), "demo/watcher");
        assert_eq!(AgentSession::canonical_id("demo/watcher#12"), "demo/watcher#12");
        assert_eq!(AgentSession::canonical_id("demo/watcher"), "demo/watcher");
        assert_eq!(
            AgentSession::split_id("projects/demo/watcher#2"),
            Some(("projects/demo".into(), "watcher".into(), 2))
        );
        assert_eq!(
            AgentSession::split_id("factory/Codex Builder"),
            Some(("factory".into(), "Codex Builder".into(), 1)),
            "a bare id is instance 1"
        );
        assert_eq!(AgentSession::split_id("watcher"), None, "no scope");
        assert_eq!(AgentSession::split_id("demo/watcher#0"), None);
        assert_eq!(AgentSession::split_id("demo/watcher#two"), None);
        assert_eq!(AgentSession::declaration_id("demo/watcher#3"), "demo/watcher");
        assert_eq!(AgentSession::declaration_id("demo/watcher"), "demo/watcher");
        assert_eq!(AgentSession::declaration_id("demo/watcher#"), "demo/watcher#");
    }

    #[test]
    fn a_connect_command_quotes_what_a_shell_would_otherwise_eat() {
        assert_eq!(AgentSession::connect_command("demo/watcher"), "factory agent attach demo/watcher");
        // zsh's extended glob and a comment would both take the `#`.
        assert_eq!(AgentSession::connect_command("demo/watcher#2"), "factory agent attach 'demo/watcher#2'");
        assert_eq!(
            AgentSession::connect_command("factory/Codex Builder"),
            "factory agent attach 'factory/Codex Builder'"
        );
        assert_eq!(AgentSession::connect_command("demo/o'brien"), "factory agent attach 'demo/o'\\''brien'");
    }

    #[test]
    fn a_stop_check_warns_only_when_something_would_be_lost() {
        let mut check = StopCheck { id: "demo/watcher#2".into(), instance: 2, live: true, reasons: Vec::new() };
        assert_eq!(check.warning(), None);
        check.reasons = vec![
            StopReason { why: KeptUp::Attached, detail: "its terminal was open in Factory 4s ago".into() },
            StopReason { why: KeptUp::Working, detail: "it is in the middle of a turn".into() },
        ];
        assert_eq!(
            check.warning().unwrap(),
            "demo/watcher#2: its terminal was open in Factory 4s ago; it is in the middle of a turn"
        );
    }

    #[test]
    fn a_scale_report_says_what_it_left_up_and_why() {
        let report = ScaleReport {
            scope: "root".into(),
            name: "claude".into(),
            previous: 3,
            wanted: 1,
            up: 2,
            started: Vec::new(),
            stopped: vec!["root/claude#2".into()],
            kept: vec![KeptInstance { id: "root/claude#3".into(), instance: 3, why: KeptUp::Working }],
        };
        assert_eq!(
            report.summary(),
            "claude in root: 1 wanted, 2 up; stopped root/claude#2; \
             root/claude#3 left up because it is working -- it stops once it is idle"
        );
    }

    #[test]
    fn standing_and_live_states_keep_their_original_vocabulary() {
        assert!(Lifetime::Permanent.is_standing() && Lifetime::Temporary.is_standing());
        assert!(!Lifetime::Task.is_standing());
        for (state, text, live) in [
            (AgentState::Starting, "starting", true),
            (AgentState::Ready, "ready", true),
            (AgentState::Gone, "gone", false),
            (AgentState::Stopped, "stopped", false),
        ] {
            assert_eq!(state.as_str(), text);
            assert_eq!(state.is_live(), live);
        }
        let agent: AgentSession = serde_json::from_value(serde_json::json!({
            "id": "demo/watch", "name": "watch", "scope": "demo",
            "agent": "shell", "runtime": "runtime", "lifetime": "permanent", "state": "ready",
            "started_at": "2026-10-04T00:00:00Z", "last_seen_at": "2026-10-04T00:00:00Z"
        }))
        .unwrap();
        assert_eq!(agent.role, Role::worker());
        assert_eq!(agent.instance, 1, "a row from before instances is instance 1");
        assert!(agent.declared);
        assert!(agent.token.is_none() && agent.assigned_role.is_none());
    }
}
