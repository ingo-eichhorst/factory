use crate::error::{FactoryError, Result};
use crate::role::{Grant, Reach, RoleDef};
use crate::task::Task;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How a runtime is to bring this agent up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchKind {
    /// A kind the runtime recognises by name and knows how to detect
    /// (`pi`, `claude`, `codex`, ...).
    Named(String),
    /// A command line to run in the session. The escape hatch that keeps
    /// plugin agents possible on a runtime with a closed list of known kinds.
    Command(Vec<String>),
}

/// What the agent adapter asks the runtime for.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub kind: LaunchKind,
    /// Extra arguments handed to the agent process.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment for the session. The callback contract travels here as well
    /// as in the prompt, so an agent can read it instead of parsing prose.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// What ties a launch to one attempt at one task. Absent when the agent is
/// being started to stand there rather than to do something.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskBinding {
    pub task: Task,
    pub run_id: String,
    pub attempt: u32,
    /// The secret the agent must present when reporting on this run.
    pub token: String,
    /// Set when this run is working in a git worktree of its own rather than
    /// the scope, so the prompt can say so and name the branch. `None` for a
    /// run that used the scope directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_branch: Option<String>,
    /// This task's direct parents in a workflow, each with the output it
    /// finished with -- empty for a root node or a task outside any
    /// workflow. Computed once, at dispatch (`Engine::dispatch`), from that
    /// moment's workflow-run state; a task not spawned by a workflow never
    /// has one to compute. `TaskBinding` crosses the out-of-process plugin
    /// protocol, so an older plugin build must still decode a binding that
    /// carries this key (`default`), and the common case -- nothing
    /// upstream -- should not put an empty array on the wire on every
    /// dispatch either (`skip_serializing_if`). See `Task::worktree` for the
    /// same reasoning applied to another field added after the wire shape
    /// already existed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream: Vec<UpstreamOutput>,
    /// The knowledge pages this run was handed, searched once at dispatch
    /// when the task has `knowledge_hints` on and something matched. `None`
    /// otherwise -- and absent on the wire then, for the same reason as
    /// `upstream`. Page ids and reasons only, never page text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<crate::adapter::knowledge::KnowledgeHints>,
    /// The steps this run must pass before its `done` counts (`#118`) --
    /// `Run::required_steps`, fixed at dispatch. Named in the reporting
    /// contract so the agent can run them itself before it says it is done.
    /// Absent on the wire when empty, like `upstream`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_steps: Vec<crate::control_plan::RequiredStep>,
}

/// One direct parent's contribution to a downstream workflow node's dispatch:
/// which node and task it came from, its title (so a prompt can name it
/// without a second lookup), and the result it finished with. `result` is
/// `None` when the parent task carries none -- reached today only if a task
/// somehow finished `Done` without ever calling `--result`, since a workflow
/// node only ever becomes a parent once its task is `Done`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamOutput {
    pub node_id: String,
    pub task_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

/// How much of a workflow parent's result a downstream dispatch keeps.
/// Applied once where `upstream` is computed (`Engine::dispatch`) and again,
/// defensively, wherever it is rendered (`HarnessAgent::prompt`), so a huge
/// result can never blow up a prompt or a task record even if some future
/// caller skips the first truncation.
pub const UPSTREAM_RESULT_BYTE_CAP: usize = 16 * 1024;

/// Keep at most `max_bytes` of `s`'s tail, marking the cut so truncated text
/// reads as a cut rather than as the whole thing. The marker is budgeted
/// inside `max_bytes` rather than added on top of it, which is what makes
/// this idempotent: truncating an already-truncated string with the same cap
/// is a no-op instead of stacking a second marker in front of a shorter and
/// shorter tail.
pub fn truncate_tail(s: &str, max_bytes: usize) -> std::borrow::Cow<'_, str> {
    if s.len() <= max_bytes {
        return std::borrow::Cow::Borrowed(s);
    }
    let marker = format!("[... truncated to the last {max_bytes} bytes ...]\n");
    if marker.len() >= max_bytes {
        // A pathologically small cap: not even the marker fits. Neither
        // caller in this codebase asks for one anywhere near this small
        // (both are tens of kilobytes), but truncating the marker itself
        // keeps "never longer than max_bytes" true regardless of that.
        let mut end = max_bytes.min(marker.len());
        while end > 0 && !marker.is_char_boundary(end) {
            end -= 1;
        }
        return std::borrow::Cow::Owned(marker[..end].to_string());
    }
    let keep = max_bytes - marker.len();
    let mut start = s.len().saturating_sub(keep);
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    std::borrow::Cow::Owned(format!("{marker}{}", &s[start..]))
}

/// Where a task run's upstream-outputs file lives, given only its task id and
/// the instance's guides directory -- the free-function twin of
/// `run_guide_path`, kept for the same reason: `close_session` needs to name
/// this file for cleanup long after the `AgentContext` that wrote it is gone.
pub fn upstream_output_path(guides_dir: &Path, task_id: &str) -> PathBuf {
    guides_dir.join(format!("upstream-{task_id}.json"))
}

/// Where a task run's knowledge-hints file lives -- `upstream_output_path`'s
/// twin, for the same reader (the shell agent) and the same cleanup.
pub fn knowledge_hints_path(guides_dir: &Path, task_id: &str) -> PathBuf {
    guides_dir.join(format!("knowledge-{task_id}.json"))
}

/// Where a run's generated shell-agent script lives, given only its run id
/// and the instance's guides directory -- the free-function twin of
/// `run_guide_path`/`upstream_output_path`, kept for the same reason:
/// `close_session` needs to name this file for cleanup long after the
/// `AgentContext` that wrote it is gone. Keyed by run id, not task id: a
/// retry's fresh run and a previous attempt's still-closing session must
/// never share -- or delete out from under each other -- the same file.
/// That is a sharper risk here than for the guide or upstream files: this
/// one is *sourced* by the pane's shell for the run's whole duration,
/// rather than read once near the start of it.
pub fn run_shell_script_path(guides_dir: &Path, run_id: &str) -> PathBuf {
    guides_dir.join(format!("shell-{run_id}.sh"))
}

/// Where a run's generated harness settings live -- for Claude Code, the
/// `--settings` file that carries its turn-end hooks. Keyed by run id for the
/// same reason as `run_shell_script_path`: the harness reads it for the
/// session's whole life, and a retry must never share or delete a previous
/// attempt's copy. A file of its own, passed by path, rather than anything
/// written into the worktree: the scope's own `.claude/settings.json` is a
/// person's, and Claude Code merges hooks from both rather than replacing
/// either.
pub fn run_hook_settings_path(guides_dir: &Path, run_id: &str) -> PathBuf {
    guides_dir.join(format!("hooks-{run_id}.json"))
}

/// Everything an agent adapter needs to phrase a prompt and a launch.
#[derive(Debug, Clone)]
pub struct AgentContext {
    pub scope: String,
    /// The concrete agent Factory knows this session as -- a declaration's
    /// name, or a bare adapter used with no declaration. Not a harness name.
    pub agent_name: String,
    /// Absolute working directory for the run.
    pub cwd: PathBuf,
    /// Absolute path to the `factory` binary the agent is to call back with.
    pub factory_bin: PathBuf,
    /// Absolute path to the daemon's control socket.
    pub socket: PathBuf,
    /// Where a guide written to a file, rather than passed as text, lives --
    /// under the instance root's `.factory/`, never inside a scope.
    pub guides_dir: PathBuf,
    /// `None` for a standing agent: it is being started to be there, and has
    /// nothing to report on.
    pub task: Option<TaskBinding>,
    /// What this agent presents to say which agent it is, when it is a standing
    /// one. A task run says so with its run token instead.
    pub identity_token: Option<String>,
    /// The role this agent is actually working under, resolved by
    /// `Engine::effective_role` at launch, the same call `caller_for` uses for
    /// every other request. `None` when the instance no longer defines the
    /// role the agent was given -- `access.rs` then refuses every grant, so
    /// the guide says so too rather than describing one that would be denied.
    pub role: Option<RoleDef>,
    /// The frameworks this agent's scope is committed to: the union of every
    /// layer's `frameworks` in `Engine::policy_chain(scope)`, resolved once
    /// at launch the same way `role` is -- never re-read once the guide is
    /// built. Empty for a scope with no `policies:` anywhere in its chain,
    /// which is the common case and not an error; `factory_guide` then says
    /// nothing about policy at all rather than naming an empty list.
    pub policy_frameworks: Vec<String>,
    /// The objective and key result a `goal=<objective>/<kr>` label names,
    /// with both titles resolved from the goals catalogue at dispatch --
    /// `None` for a task carrying no such label, or naming one the
    /// catalogue does not (currently) define. Resolved once at launch, the
    /// same way `policy_frameworks` is, never re-read once the guide is
    /// built.
    pub goal: Option<GoalContext>,
    /// The H-importance quality attributes this agent's scope declares
    /// (`#107`), each with how its scenarios are measured and, when one is
    /// not met, its status -- resolved once at launch from the scope's
    /// merged utility tree, the same way `policy_frameworks` is. Empty for
    /// a scope binding no quality profile, or none ranked H, and then
    /// `factory_guide` says nothing about quality at all.
    pub quality: Vec<QualityAttributeContext>,
}

/// One H-importance quality attribute, as `AgentContext::factory_guide`
/// names it: the attribute id and each of its scenarios in one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityAttributeContext {
    pub attribute: String,
    pub scenarios: Vec<QualityScenarioContext>,
}

/// One scenario under a [`QualityAttributeContext`]. Carries only what
/// changes when something real changes -- the measure's own wording and a
/// status word -- never a metric's current value or when it was read, so
/// the guide stays byte-for-byte the same from one dispatch to the next
/// while nothing about the scope's quality moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualityScenarioContext {
    pub scenario: String,
    /// `quality::describe_measure`'s one line; `None` for a draft.
    pub measure: Option<String>,
    /// `not met`, `stale` or `no data` -- `None` when the scenario is met
    /// (nothing to say) or a draft (`measure: None` already says it).
    pub status: Option<String>,
}

/// What `AgentContext::factory_guide` says about a task's `goal=` label:
/// the objective and key result it names, already resolved to their
/// authored titles so the guide never has to repeat raw ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalContext {
    pub objective_id: String,
    pub objective_title: String,
    pub kr_id: String,
    pub kr_title: String,
}

/// Where a task run's guide file lives, given only its task id and the
/// instance's guides directory. A free function rather than only a method on
/// `AgentContext` so a caller that has neither -- `close_session`, ending a
/// run after the launch that built the context is long gone -- can name the
/// exact same path without rebuilding the naming scheme by hand and risking
/// the two drifting apart. `AgentContext::guide_path` delegates here for the
/// task case; keep it that way rather than duplicating the format string.
pub fn run_guide_path(guides_dir: &Path, task_id: &str) -> PathBuf {
    guides_dir.join(format!("run-{task_id}.md"))
}

impl AgentContext {
    pub fn binding(&self) -> Result<&TaskBinding> {
        self.task.as_ref().ok_or_else(|| {
            FactoryError::BadRequest(
                "this agent was started without a task, so there is nothing to say to it".into(),
            )
        })
    }

    /// The reporting contract, in the words every built-in agent puts in its
    /// prompt. Adapters are free to phrase it differently, but the commands
    /// have to be these.
    pub fn reporting_contract(&self) -> String {
        let Some(binding) = &self.task else {
            return String::new();
        };
        let bin = self.factory_bin.display();
        let id = &binding.task.id;
        let checks: Vec<String> = binding
            .required_steps
            .iter()
            .filter(|s| s.kind.enforced())
            .map(|s| {
                let command = s.command.as_deref().map(|c| format!(" (`{c}`)")).unwrap_or_else(|| " (no command declared)".into());
                let by = if s.required_by.is_empty() { String::new() } else { format!(", required by {}", s.required_by.join(", ")) };
                format!("- {}{command}{by}\n", s.step)
            })
            .collect();
        let verification = if checks.is_empty() {
            String::new()
        } else {
            format!(
                "\n\nBefore done counts, Factory runs these checks itself, in your working \
                 directory, and the run waits in `verifying` until they pass:\n{}\
                 Run them yourself before you report done. If one fails, the run is blocked \
                 with the reason; fix it and report done again.",
                checks.concat()
            )
        };
        let contract = format!(
            "Report progress by running these commands in your shell. They are how \
             this task is tracked; nothing watches your terminal to guess.\n\
             \n\
             - Starting work:   {bin} task report {id} --status running --message \"<what you are doing>\"\n\
             - A useful update: {bin} task report {id} --message \"<the update>\"\n\
             - Need a human:    {bin} task report {id} --status blocked --message \"<what you need>\"\n\
             - Finished:        {bin} task report {id} --status done --result \"<what you did>\"\n\
             - Gave up:         {bin} task report {id} --status failed --error \"<why>\"\n\
             \n\
             Report running first, then finish with exactly one of done, failed, or \
             blocked -- before your turn ends. When your harness says a turn ended \
             without one of them, and nothing is still running in the background to \
             wake you, Factory may end the run as failed."
        );
        contract + &verification
    }

    /// A short, adapter-neutral guide to Factory itself: what it is, who this
    /// agent is, how to call it, and exactly the commands its role allows.
    /// Adapters put this in the agent's *system* prompt rather than the task
    /// prompt, so it survives however long the session runs and whatever of
    /// the task prompt gets compacted away. It deliberately says nothing
    /// about reporting: `reporting_contract()` already carries those exact
    /// commands, in the task prompt, and this points there instead of saying
    /// them a second time in a second place.
    pub fn factory_guide(&self) -> String {
        let mut out = String::new();

        out.push_str(
            "You are running inside Factory, a daemon that hands tasks to \
             coding agents and tracks what happens. A scope is a place work \
             happens -- a project with its own agents and configuration. A \
             task is the standing intent: what to do, and with which agent; \
             a run is one attempt at it, with its own session and outcome. \
             Status comes only from an agent calling `task report`, never \
             from what a terminal looks like.\n\n",
        );

        match &self.task {
            Some(binding) => out.push_str(&format!(
                "You are {name}, in scope {scope}, working task {task_id} \
                 (\"{title}\"). This session ends with the run.\n\n",
                name = self.agent_name,
                scope = self.scope,
                task_id = binding.task.id,
                title = binding.task.title,
            )),
            None => out.push_str(&format!(
                "You are {name}, a standing agent in scope {scope}: you were \
                 started to be here, not to finish something. If nobody has \
                 given you a task, that is the job -- sit quiet and wait \
                 rather than looking for work.\n\n",
                name = self.agent_name,
                scope = self.scope,
            )),
        }

        match &self.role {
            Some(def) => {
                let reach = match def.reach {
                    Reach::Own => {
                        "your own work only -- tasks assigned to you, the run you hold, yourself"
                    }
                    Reach::Scope => "everything in your scope, and nothing past it",
                };
                out.push_str(&format!(
                    "Your role is {role} ({describe}). Its reach is {reach}. \
                     A role describes your job; it is not a lock, so follow \
                     it rather than test it. If a command is refused, the \
                     reply names the grant you are missing -- that is the \
                     answer, not a reason to try again a different way. \
                     `factory agent role` can hand you a different role \
                     while you run; `factory agents` shows the one actually \
                     in effect right now.\n\n",
                    role = def.name,
                    describe = def.describe,
                    reach = reach,
                ));
            }
            None => out.push_str(
                "You were given a role this instance no longer defines, so \
                 none of the write commands below are yours -- only reading \
                 is. `factory agents` shows the roles this instance knows.\n\n",
            ),
        }

        let bin = self.factory_bin.display();
        out.push_str(&format!(
            "Call Factory as {bin} -- the absolute path, so a `factory` \
             missing from your PATH does not matter. FACTORY_TOKEN is \
             already set in your environment and sent on every request, so \
             you never need to pass --token yourself. Add --json to any \
             command for output meant to be parsed rather than read.\n\n\
             Reading is open to every agent, whatever your role: {bin} task \
             list, task show <id>, task log <id>, task output <id>; {bin} \
             agents; {bin} run list [task-id], run show <run-id>, run log \
             <run-id>, run output <run-id>; {bin} adapters; {bin} infra, \
             which shows the host, the daemon and which AI account each \
             agent's model calls go to.\n\n\
             The company's knowledge base is part of that: {bin} knowledge \
             search <words> [--tag <tag>] lists the pages that match, best \
             first, as file paths with the reason each matched. It never \
             prints a page's text -- open the file to read it. Worth a look \
             before you start on anything about a client, a product or a \
             process the company already has.\n\n",
        ));

        if !self.policy_frameworks.is_empty() {
            out.push_str(&format!(
                "Your scope is committed to {frameworks} -- {bin} policy \
                 status --scope {scope} shows every control and where each \
                 one stands.\n\n",
                frameworks = self.policy_frameworks.join(", "),
                scope = self.scope,
            ));
        }

        if let Some(goal) = &self.goal {
            out.push_str(&format!(
                "This task serves objective \"{objective}\" ({objective_id}), key result \
                 \"{kr}\" ({objective_id}/{kr_id}) -- goals enforce nothing, so this is context, \
                 not an instruction on top of your own.\n\n",
                objective = goal.objective_title,
                objective_id = goal.objective_id,
                kr = goal.kr_title,
                kr_id = goal.kr_id,
            ));
        }

        if !self.quality.is_empty() {
            out.push_str(&format!(
                "What \"good\" means in {scope}: these quality attributes are ranked H \
                 importance here, each with how it is measured. They are a picture, not a \
                 gate -- nothing is blocked on them -- but a change that makes one worse \
                 should say so. {bin} quality scope {scope} shows every scenario and its \
                 evidence.\n",
                scope = self.scope,
            ));
            for attr in &self.quality {
                let scenarios: Vec<String> = attr
                    .scenarios
                    .iter()
                    .map(|s| {
                        let measure = s.measure.as_deref().unwrap_or("no measure yet");
                        match &s.status {
                            Some(status) => format!("{}: {measure} [{status}]", s.scenario),
                            None => format!("{}: {measure}", s.scenario),
                        }
                    })
                    .collect();
                out.push_str(&format!("- {}: {}\n", attr.attribute, scenarios.join("; ")));
            }
            out.push('\n');
        }

        if let Some(def) = &self.role {
            let lines = self.granted_command_lines(def);
            if !lines.is_empty() {
                out.push_str("Your role also lets you:\n");
                for line in lines {
                    out.push_str(&format!("- {line}\n"));
                }
                out.push('\n');
            }
        }

        out.push_str(
            "Your reach stops where the boundary above says it does. Never \
             write anything of Factory's own -- like this guide -- into a \
             scope; a scope owns only its own config.yaml, and everything \
             else Factory keeps stays under the instance root's .factory/.",
        );

        out
    }

    /// One line per grant this role holds, each mapped to the command that
    /// exercises it. A grant with no command of its own yet -- reachable
    /// today only through the web UI -- says so honestly rather than
    /// inventing one.
    fn granted_command_lines(&self, def: &RoleDef) -> Vec<String> {
        let bin = self.factory_bin.display();
        let scope = &self.scope;
        let mut lines = Vec::new();
        for grant in Grant::ALL {
            if !def.allows(grant) {
                continue;
            }
            lines.push(match grant {
                Grant::TaskCreate => format!(
                    "task.create -> {bin} task create \"<title>\" -i \"<instructions>\" --scope {scope} --agent <agent>"
                ),
                Grant::TaskEdit => {
                    format!("task.edit -> {bin} task edit <id> ... (see --help for every field)")
                }
                Grant::TaskDelete => format!("task.delete -> {bin} task delete <id>"),
                Grant::TaskRun => format!("task.run -> {bin} task run <id>"),
                Grant::TaskCancel => format!("task.cancel -> {bin} task cancel <id>"),
                Grant::TaskReport => match &self.task {
                    // Not repeated here: the exact commands for this run are
                    // already in the task prompt.
                    Some(_) => {
                        "task.report -> the exact commands are in your task prompt, not repeated here".to_string()
                    }
                    None => format!(
                        "task.report -> {bin} task report <id> --status <running|done|failed|blocked> --message/--result/--error \"...\", once you are given a task to report on"
                    ),
                },
                Grant::AgentStart => format!("agent.start -> {bin} agent start {scope} <name>"),
                Grant::AgentConfigure => {
                    "agent.configure -> add, edit or delete a standing agent's declaration in its scope; today that is the web UI's Roster, not this CLI".to_string()
                }
                Grant::AgentStop => format!("agent.stop -> {bin} agent stop <id>"),
                Grant::AgentInput => {
                    format!("agent.input -> {bin} agent input <id> --text \"...\" --key enter")
                }
                Grant::RunInput => {
                    "run.input -> type into a run's own terminal; today that is the web UI, not this CLI".to_string()
                }
                Grant::WorkflowCreate => {
                    "workflow.create -> create a workflow in your scope; today that is the web UI's Process > Workflows view or POST /api/workflows, not this CLI".to_string()
                }
                Grant::WorkflowEdit => {
                    "workflow.edit -> change a workflow in your scope; today that is the web UI's Process > Workflows view or PATCH /api/workflows/<id>, not this CLI".to_string()
                }
                Grant::WorkflowDelete => {
                    "workflow.delete -> delete a workflow in your scope; today that is the web UI's Process > Workflows view or DELETE /api/workflows/<id>, not this CLI".to_string()
                }
                Grant::WorkflowRun => {
                    "workflow.run -> start a workflow in your scope, which also needs task.create and task.run for every node it spawns; today that is the web UI or POST /api/workflows/<id>/run, not this CLI".to_string()
                }
                Grant::WorkflowCancel => {
                    "workflow.cancel -> cancel a workflow run in your scope; today that is the web UI or POST /api/workflow-runs/<id>/cancel, not this CLI".to_string()
                }
                Grant::KnowledgeWrite => format!(
                    "knowledge.write -> {bin} knowledge import <dir> or {bin} knowledge add <file>...; the knowledge base is company-wide, not scoped to {scope}"
                ),
                Grant::DatasetEdit => format!(
                    "dataset.edit -> {bin} dataset create|case add|case rm|rm|import|from-tasks ..."
                ),
                Grant::BenchRun => format!(
                    "bench.run -> {bin} bench run <dataset> --agent <scope>/<agent> [--attempts N] [--concurrency N]; also bench cancel/clean <run-id>"
                ),
                Grant::PolicyAttest => format!(
                    "policy.attest -> {bin} policy attest <framework>/<id> --scope <scope> --evidence <pointer> --expires <30d|2027-01-01> [--note \"...\"]; also {bin} policy withdraw <attestation-id> --reason \"...\"; the subject is whichever scope the attestation is recorded for, but the grant itself is company-wide, not scoped to {scope}"
                ),
                Grant::GoalsCheckIn => format!(
                    "goals.checkin -> {bin} goals checkin <objective>/<kr> --value <n> --confidence <0-10> [--note \"...\"]; only for a manual key result, and the grant itself is company-wide, not scoped to {scope}"
                ),
            });
        }
        lines
    }

    /// Where this session's guide file would live, whether or not anything
    /// has written it yet. Named per task, not per run: only one run of a
    /// task is ever in progress at a time, so a retry safely overwrites
    /// rather than leaving the last attempt's file behind. A standing
    /// agent's is named for its own identity, so restarting it overwrites
    /// the same file instead of piling up a new one.
    pub fn guide_path(&self) -> PathBuf {
        match &self.task {
            Some(binding) => run_guide_path(&self.guides_dir, &binding.task.id),
            None => self
                .guides_dir
                .join(format!("agent-{}-{}.md", self.scope, self.agent_name)),
        }
    }

    /// Write the guide to its file, for a harness whose own system-prompt
    /// mechanism wants a path rather than inline text. Overwrites whatever
    /// was there, per `guide_path`'s naming.
    pub fn write_guide_file(&self) -> Result<PathBuf> {
        let path = self.guide_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("making {}: {e}", parent.display())))?;
        }
        std::fs::write(&path, self.factory_guide())
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("writing {}: {e}", path.display())))?;
        Ok(path)
    }

    /// Write this run's knowledge hints to their file, for an agent (the
    /// shell agent) that would rather be pointed at a path than have them
    /// spliced into a typed line. `None` when the run was handed none.
    /// Idempotent, like `write_upstream_file`, and for the same reason.
    pub fn write_knowledge_file(&self) -> Result<Option<PathBuf>> {
        let Some(binding) = self.task.as_ref() else {
            return Ok(None);
        };
        let Some(hints) = &binding.knowledge else {
            return Ok(None);
        };
        let path = knowledge_hints_path(&self.guides_dir, &binding.task.id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("making {}: {e}", parent.display())))?;
        }
        let json = serde_json::to_string_pretty(hints)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("encoding knowledge hints: {e}")))?;
        std::fs::write(&path, json)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("writing {}: {e}", path.display())))?;
        Ok(Some(path))
    }

    /// Where this run's upstream-outputs file would live, if it has anything
    /// to write -- `None` for a root node or a task outside any workflow,
    /// same check `write_upstream_file` makes, so a caller can name the path
    /// without writing anything first.
    pub fn upstream_path(&self) -> Option<PathBuf> {
        let binding = self.task.as_ref()?;
        if binding.upstream.is_empty() {
            return None;
        }
        Some(upstream_output_path(&self.guides_dir, &binding.task.id))
    }

    /// Write this run's upstream outputs to their file, for an agent (the
    /// shell agent, today) that would rather point a command at a path than
    /// have a parent's stdout -- quotes, `$`, backticks, newlines and all --
    /// spliced into a typed line. `None` when there is nothing to write
    /// rather than an empty file, so a caller can tell "no parents" from
    /// "write failed". Idempotent: called from both `launch_spec` (so the
    /// path is real before `FACTORY_UPSTREAM_FILE` is exported) and `prompt`
    /// (so the inline `export` in the typed line does not depend on that
    /// order), and overwriting the same content twice is harmless.
    pub fn write_upstream_file(&self) -> Result<Option<PathBuf>> {
        let Some(path) = self.upstream_path() else {
            return Ok(None);
        };
        let upstream = &self.binding()?.upstream;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("making {}: {e}", parent.display())))?;
        }
        let json = serde_json::to_string_pretty(upstream)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("encoding upstream outputs: {e}")))?;
        std::fs::write(&path, json)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("writing {}: {e}", path.display())))?;
        Ok(Some(path))
    }

    /// Where this run's shell-agent script would live. `None` only for a
    /// standing agent, which has no run to key a script by.
    pub fn shell_script_path(&self) -> Option<PathBuf> {
        self.task
            .as_ref()
            .map(|b| run_shell_script_path(&self.guides_dir, &b.run_id))
    }

    /// Write the shell agent's whole command wrapper -- the running/done/
    /// failed report calls, the stdout capture, and cleanup -- to its file,
    /// so the line actually typed into the pane can be a short `. '<path>'`
    /// instead of carrying all of that itself. See `ShellAgent::prompt`'s
    /// own comment for why that matters.
    pub fn write_shell_script(&self, contents: &str) -> Result<PathBuf> {
        let path = self.shell_script_path().ok_or_else(|| {
            FactoryError::BadRequest(
                "this agent was started without a task, so there is no run to script for".into(),
            )
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("making {}: {e}", parent.display())))?;
        }
        std::fs::write(&path, contents)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("writing {}: {e}", path.display())))?;
        Ok(path)
    }

    /// Write a run's harness settings file (see `run_hook_settings_path`).
    /// `None` for a standing agent: there is no run for a turn to end.
    pub fn write_hook_settings(&self, contents: &str) -> Result<Option<PathBuf>> {
        let Some(binding) = &self.task else {
            return Ok(None);
        };
        let path = run_hook_settings_path(&self.guides_dir, &binding.run_id);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("making {}: {e}", parent.display())))?;
        }
        std::fs::write(&path, contents)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("writing {}: {e}", path.display())))?;
        Ok(Some(path))
    }

    /// The same contract as environment, for adapters that would rather read it.
    pub fn env(&self) -> BTreeMap<String, String> {
        let mut env = BTreeMap::from([
            ("FACTORY_SCOPE".to_string(), self.scope.clone()),
            ("FACTORY_SOCKET".to_string(), self.socket.display().to_string()),
            ("FACTORY_BIN".to_string(), self.factory_bin.display().to_string()),
        ]);
        // One variable says which agent is calling, whether it is standing
        // there or working a task. The CLI sends it on every request.
        if let Some(token) = self
            .task
            .as_ref()
            .map(|b| b.token.clone())
            .or_else(|| self.identity_token.clone())
        {
            env.insert("FACTORY_TOKEN".into(), token);
        }
        if let Some(b) = &self.task {
            env.insert("FACTORY_TASK_ID".into(), b.task.id.clone());
            env.insert("FACTORY_TASK_TOKEN".into(), b.token.clone());
            env.insert("FACTORY_RUN_ID".into(), b.run_id.clone());
            env.insert("FACTORY_RUN_ATTEMPT".into(), b.attempt.to_string());
        }
        env
    }
}

/// Adapter seam 1: a coding agent. Knows how its harness is started and how to
/// say a task to it.
#[async_trait::async_trait]
pub trait Agent: Send + Sync {
    fn name(&self) -> &str;

    /// One line for `factory adapters`.
    fn description(&self) -> String {
        format!("{} agent", self.name())
    }

    /// How to bring this agent up. Called for a task run and for a standing
    /// agent alike; `ctx.task` says which.
    async fn launch_spec(&self, ctx: &AgentContext) -> Result<LaunchSpec>;

    /// The text submitted to the agent once it is up. Only called when there
    /// is a task: a standing agent is started and then left alone.
    async fn prompt(&self, ctx: &AgentContext) -> Result<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role::Roles;
    use crate::task::{Task, TaskStatus};
    use std::collections::BTreeSet;

    fn task() -> Task {
        let now = chrono::Utc::now();
        Task {
            id: "t1".into(),
            title: "fix the flaky test".into(),
            instructions: String::new(),
            scope: "demo".into(),
            agent: "watcher".into(),
            runtime: "herdr".into(),
            status: TaskStatus::Dispatching,
            schedule: None,
            estimate_seconds: None,
            result: None,
            error: None,
            runs: 1,
            ack_timeout_seconds: None,
            timeout_seconds: None,
            blocked_timeout_seconds: None,
            worktree: false,
            knowledge_hints: false,
            labels: Default::default(),
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at: None,
            workflow_origin: None,
            bench_origin: None,
            retry: None,
            pending_retry: None,
            schedule_paused: false,
            category: None,
        }
    }

    fn base(role: Option<RoleDef>) -> AgentContext {
        AgentContext {
            scope: "demo".into(),
            agent_name: "watcher".into(),
            cwd: PathBuf::from("/tmp/somewhere"),
            factory_bin: PathBuf::from("/usr/local/bin/factory"),
            socket: PathBuf::from("/tmp/factory.sock"),
            guides_dir: PathBuf::from("/tmp/factory-guides"),
            task: None,
            identity_token: Some("identity".into()),
            role,
            policy_frameworks: Vec::new(),
            goal: None,
            quality: Vec::new(),
        }
    }

    fn with_task(mut ctx: AgentContext) -> AgentContext {
        ctx.task = Some(TaskBinding {
            task: task(),
            run_id: "r1".into(),
            attempt: 1,
            token: "tok".into(),
            worktree_branch: None,
            upstream: Vec::new(),
            knowledge: None,
            required_steps: Vec::new(),
        });
        ctx.identity_token = None;
        ctx
    }

    fn worker() -> RoleDef {
        Roles::presets().get(&crate::role::Role::worker()).unwrap().clone()
    }

    fn foreman() -> RoleDef {
        Roles::presets().get(&crate::role::Role::foreman()).unwrap().clone()
    }

    fn custom(grants: &[Grant], reach: Reach) -> RoleDef {
        RoleDef {
            name: crate::role::Role::new("runner"),
            describe: "starts what is already on the board".into(),
            grants: grants.iter().copied().collect::<BTreeSet<_>>(),
            reach,
        }
    }

    #[test]
    fn a_worker_is_never_told_it_can_create_tasks() {
        let guide = base(Some(worker())).factory_guide();
        assert!(guide.contains("task.edit"));
        assert!(guide.contains("run.input"));
        assert!(!guide.contains("task.create"), "a worker cannot create tasks: {guide}");
        assert!(!guide.contains("agent.start"));
    }

    #[test]
    fn a_foreman_is_told_it_can_create_edit_and_assign_tasks_in_its_scope() {
        let guide = base(Some(foreman())).factory_guide();
        for grant in ["task.create", "task.edit", "task.delete", "task.run", "task.cancel", "agent.start"] {
            assert!(guide.contains(grant), "a foreman has {grant}: {guide}");
        }
        assert!(guide.contains("everything in your scope"));
    }

    #[test]
    fn a_custom_role_gets_exactly_its_grants_and_nothing_the_grants_do_not_cover() {
        let guide = base(Some(custom(&[Grant::TaskRun, Grant::TaskCancel], Reach::Scope))).factory_guide();
        assert!(guide.contains("task.run"));
        assert!(guide.contains("task.cancel"));
        assert!(!guide.contains("task.create"));
        assert!(!guide.contains("task.edit"));
        assert!(!guide.contains("agent.input"));
    }

    #[test]
    fn a_role_this_instance_no_longer_defines_claims_no_grants() {
        // `access.rs` refuses every grant when `Roles::get` finds nothing for
        // the role an agent is holding; the guide must not promise more than
        // that check would allow, so it says nothing under "your role also
        // lets you" rather than guessing.
        let guide = base(None).factory_guide();
        assert!(!guide.contains("Your role also lets you"));
        assert!(guide.contains("no longer defines"));
    }

    #[test]
    fn every_agent_is_told_it_can_search_the_knowledge_base_whatever_its_role() {
        // Searching is a read, open to every agent -- so it is named in the
        // reading sentence, not among the grants, and a role with no grants
        // at all (or one the instance no longer defines) still hears of it.
        for role in [Some(worker()), Some(custom(&[], Reach::Own)), None] {
            let guide = base(role).factory_guide();
            assert!(guide.contains("knowledge search <words>"), "{guide}");
            assert!(guide.contains("never prints a page's text"), "{guide}");
        }
    }

    #[test]
    fn the_guide_names_the_frameworks_a_scope_with_policies_has() {
        let mut ctx = base(Some(worker()));
        ctx.policy_frameworks = vec!["cra".into(), "gdpr".into()];
        let guide = ctx.factory_guide();
        assert!(guide.contains("cra, gdpr"), "{guide}");
        assert!(guide.contains("policy status --scope demo"), "{guide}");
    }

    #[test]
    fn the_guide_says_nothing_about_policy_for_a_scope_with_none() {
        let guide = base(Some(worker())).factory_guide();
        assert!(
            !guide.contains("policy status"),
            "no policies apply, so nothing should point at the command: {guide}"
        );
    }

    #[test]
    fn the_guide_names_the_objective_and_key_result_a_goal_label_resolved_to() {
        let mut ctx = base(Some(worker()));
        ctx.goal = Some(GoalContext {
            objective_id: "ship-compliant".into(),
            objective_title: "Every product can ship CRA-compliant".into(),
            kr_id: "cra-open-zero".into(),
            kr_title: "No open CRA controls".into(),
        });
        let guide = ctx.factory_guide();
        assert!(guide.contains("Every product can ship CRA-compliant"), "{guide}");
        assert!(guide.contains("No open CRA controls"), "{guide}");
        assert!(guide.contains("ship-compliant/cra-open-zero"), "{guide}");
    }

    #[test]
    fn the_guide_says_nothing_about_a_goal_when_the_task_carries_no_label() {
        let guide = base(Some(worker())).factory_guide();
        assert!(!guide.contains("This task serves objective"), "{guide}");
    }

    fn quality_context() -> Vec<QualityAttributeContext> {
        vec![
            QualityAttributeContext {
                attribute: "reliability.recoverability".into(),
                scenarios: vec![
                    QualityScenarioContext {
                        scenario: "daemon-restart".into(),
                        measure: Some("scrap_rate <= 0.05 within 7d".into()),
                        status: Some("not met".into()),
                    },
                    QualityScenarioContext {
                        scenario: "little-work-scrapped".into(),
                        measure: Some("scrap_rate <= 0.1".into()),
                        status: None,
                    },
                ],
            },
            QualityAttributeContext {
                attribute: "security.confidentiality".into(),
                scenarios: vec![QualityScenarioContext {
                    scenario: "agents-sandboxed".into(),
                    measure: None,
                    status: None,
                }],
            },
        ]
    }

    /// The quality block exactly, as a snapshot: the one place its wording
    /// is pinned, so a change to it is a change someone meant to make.
    #[test]
    fn the_guide_names_a_scopes_h_importance_quality_attributes_one_line_each() {
        let mut ctx = base(Some(worker()));
        ctx.quality = quality_context();
        let guide = ctx.factory_guide();
        let expected = "What \"good\" means in demo: these quality attributes are ranked H \
             importance here, each with how it is measured. They are a picture, not a gate -- \
             nothing is blocked on them -- but a change that makes one worse should say so. \
             /usr/local/bin/factory quality scope demo shows every scenario and its evidence.\n\
             - reliability.recoverability: daemon-restart: scrap_rate <= 0.05 within 7d [not met]; \
             little-work-scrapped: scrap_rate <= 0.1\n\
             - security.confidentiality: agents-sandboxed: no measure yet\n\n";
        assert!(guide.contains(expected), "{guide}");
    }

    #[test]
    fn the_quality_block_is_byte_stable_and_absent_when_nothing_is_ranked_h() {
        let mut ctx = base(Some(worker()));
        ctx.quality = quality_context();
        assert_eq!(ctx.factory_guide(), ctx.clone().factory_guide());
        let guide = base(Some(worker())).factory_guide();
        assert!(!guide.contains("quality"), "{guide}");
    }

    #[test]
    fn the_guide_never_repeats_the_reporting_contract() {
        let ctx = with_task(base(Some(worker())));
        let guide = ctx.factory_guide();
        let contract = ctx.reporting_contract();
        assert!(!contract.is_empty());
        // The reporting contract's own distinctive phrasing must appear only
        // in the contract, not duplicated into the guide.
        assert!(!guide.contains("<what you are doing>"));
        assert!(!guide.contains("Gave up:"));
        assert!(guide.contains("task prompt"), "it points at the contract instead: {guide}");
    }

    #[test]
    fn a_standing_agent_holding_task_report_gets_a_generic_form_since_it_has_no_task_prompt() {
        let guide = base(Some(worker())).factory_guide();
        assert!(!guide.contains("task prompt"), "there is no task prompt to point at: {guide}");
        assert!(guide.contains("task.report"));
        assert!(guide.contains("factory task report"));
    }

    #[test]
    fn who_you_are_differs_for_a_standing_agent_and_a_task_run() {
        let standing = base(Some(worker())).factory_guide();
        assert!(standing.contains("a standing agent"));
        assert!(standing.contains("sit quiet and wait"));

        let running = with_task(base(Some(worker()))).factory_guide();
        assert!(running.contains("working task t1"));
        assert!(!running.contains("a standing agent"));
    }

    #[test]
    fn the_guide_never_names_a_harness_or_herdr() {
        // Adapter-neutral: every harness injects the same text.
        let guide = with_task(base(Some(foreman()))).factory_guide().to_lowercase();
        for phrase in ["herdr", "claude", "codex", "opencode"] {
            assert!(!guide.contains(phrase), "{phrase} should not appear: {guide}");
        }
        let words: BTreeSet<&str> = guide
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        assert!(!words.contains("pi"), "pi should not appear as its own word: {guide}");
    }

    #[test]
    fn guide_paths_are_named_per_task_for_a_run_and_per_identity_for_a_standing_agent() {
        let standing = base(None);
        assert_eq!(standing.guide_path(), PathBuf::from("/tmp/factory-guides/agent-demo-watcher.md"));

        let running = with_task(base(None));
        assert_eq!(running.guide_path(), PathBuf::from("/tmp/factory-guides/run-t1.md"));
    }

    #[test]
    fn a_task_runs_guide_path_agrees_with_the_free_function_a_caller_with_no_context_would_use() {
        // `close_session`, ending a run well after the `AgentContext` that
        // launched it is gone, has only a task id and the guides directory
        // to work with. It must land on exactly the path `guide_path` built,
        // or it deletes nothing and a run leaks a file. Delegation to
        // `run_guide_path`, pinned here, is what keeps that true by
        // construction rather than by two format strings staying in sync.
        let running = with_task(base(None));
        assert_eq!(
            running.guide_path(),
            run_guide_path(&PathBuf::from("/tmp/factory-guides"), "t1")
        );
    }

    // -- upstream outputs -----------------------------------------------

    /// A fresh, real directory every call. `base()`'s guides_dir is a fixed
    /// path shared by every test in this module that doesn't touch the
    /// filesystem; a test that actually writes and reads a file back needs
    /// one of its own, or it races another test using the very same path.
    fn fresh_guides_dir() -> PathBuf {
        std::env::temp_dir().join(format!("factory-core-agent-test-{}", uuid::Uuid::new_v4()))
    }

    fn upstream(entries: Vec<UpstreamOutput>) -> AgentContext {
        let mut ctx = with_task(base(None));
        ctx.guides_dir = fresh_guides_dir();
        ctx.task.as_mut().unwrap().upstream = entries;
        ctx
    }

    fn one_output() -> UpstreamOutput {
        UpstreamOutput {
            node_id: "a".into(),
            task_id: "ta".into(),
            title: "build it".into(),
            result: Some("ok".into()),
        }
    }

    #[test]
    fn a_task_with_no_upstream_gets_no_file_and_no_path() {
        let ctx = with_task(base(None));
        assert_eq!(ctx.upstream_path(), None);
        assert_eq!(ctx.write_upstream_file().unwrap(), None);
        assert!(!ctx.guides_dir.exists(), "nothing at all was written");
    }

    #[test]
    fn a_standing_agent_has_no_upstream_path_either() {
        // No task at all means no binding to read `upstream` off of.
        let ctx = base(None);
        assert_eq!(ctx.upstream_path(), None);
        assert_eq!(ctx.write_upstream_file().unwrap(), None);
    }

    #[test]
    fn a_task_with_upstream_writes_a_json_file_named_by_task_id() {
        let ctx = upstream(vec![one_output()]);
        let expected = upstream_output_path(&ctx.guides_dir, "t1");
        assert_eq!(ctx.upstream_path(), Some(expected.clone()));
        let path = ctx.write_upstream_file().unwrap().expect("something to write");
        assert_eq!(path, expected);
        let written: Vec<UpstreamOutput> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0].node_id, "a");
        assert_eq!(written[0].result.as_deref(), Some("ok"));
        std::fs::remove_dir_all(&ctx.guides_dir).ok();
    }

    #[test]
    fn writing_the_upstream_file_twice_is_harmless() {
        // `launch_spec` and `prompt` each call this independently rather than
        // relying on one having already run; the second write must not fail
        // or change the outcome.
        let ctx = upstream(vec![one_output()]);
        let first = ctx.write_upstream_file().unwrap().unwrap();
        let second = ctx.write_upstream_file().unwrap().unwrap();
        assert_eq!(first, second);
        std::fs::remove_dir_all(&ctx.guides_dir).ok();
    }

    // -- the shell agent's generated script ------------------------------

    fn with_fresh_guides_dir() -> AgentContext {
        let mut ctx = with_task(base(None));
        ctx.guides_dir = fresh_guides_dir();
        ctx
    }

    #[test]
    fn a_standing_agent_has_no_shell_script_path() {
        let ctx = base(None);
        assert_eq!(ctx.shell_script_path(), None);
        assert!(ctx.write_shell_script("echo hi").is_err());
    }

    #[test]
    fn a_shell_script_is_named_by_run_id_not_task_id() {
        let ctx = with_fresh_guides_dir();
        let expected = run_shell_script_path(&ctx.guides_dir, "r1");
        assert_eq!(ctx.shell_script_path(), Some(expected.clone()));
        let path = ctx.write_shell_script("echo hi").unwrap();
        assert_eq!(path, expected);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "echo hi");
        std::fs::remove_dir_all(&ctx.guides_dir).ok();
    }

    #[test]
    fn two_runs_of_the_same_task_get_two_different_script_files() {
        // The whole point of keying by run id: a retry must never collide
        // with -- or have its script deleted by a late cleanup of -- an
        // earlier attempt's file.
        let mut first = with_fresh_guides_dir();
        first.guides_dir = fresh_guides_dir();
        let mut second = first.clone();
        second.task.as_mut().unwrap().run_id = "r2".into();

        let first_path = first.write_shell_script("attempt one").unwrap();
        let second_path = second.write_shell_script("attempt two").unwrap();
        assert_ne!(first_path, second_path);
        assert_eq!(std::fs::read_to_string(&first_path).unwrap(), "attempt one");
        assert_eq!(std::fs::read_to_string(&second_path).unwrap(), "attempt two");
        std::fs::remove_dir_all(&first.guides_dir).ok();
    }

    #[test]
    fn truncate_tail_keeps_the_end_and_marks_the_cut() {
        let long = "a".repeat(100);
        let kept = truncate_tail(&long, 80);
        assert!(kept.len() <= 80, "never longer than the cap: {} bytes", kept.len());
        assert!(kept.ends_with('a'), "the tail, not the head, survives: {kept}");
        assert!(kept.contains("truncated"), "the cut is marked: {kept}");
    }

    #[test]
    fn truncate_tail_never_exceeds_the_cap_even_when_the_marker_alone_would_not_fit() {
        // Neither real caller asks for anything near this small, but the
        // invariant -- never longer than `max_bytes` -- has to hold even
        // when the marker text itself does not fit in the budget.
        let long = "a".repeat(100);
        let kept = truncate_tail(&long, 10);
        assert!(kept.len() <= 10, "never longer than the cap: {} bytes", kept.len());
    }

    #[test]
    fn truncate_tail_is_a_no_op_under_the_cap() {
        let short = "hello";
        assert_eq!(truncate_tail(short, 40), std::borrow::Cow::Borrowed(short));
    }

    #[test]
    fn truncate_tail_never_splits_a_multibyte_character() {
        let s = "é".repeat(30); // each 'é' is 2 bytes in UTF-8
        // The byte budget alone lands mid-character; slicing there would
        // have panicked outright if the boundary walk were missing.
        let kept = truncate_tail(&s, 60);
        assert!(kept.ends_with('é'), "the kept tail is whole characters, not a stray byte: {kept:?}");
    }

    #[test]
    fn truncate_tail_is_idempotent() {
        // The marker is budgeted inside the cap rather than added on top of
        // it, precisely so a second pass (the defensive one in a harness
        // prompt's rendering, after the first at dispatch) is a no-op rather
        // than stacking a second marker in front of an ever-shorter tail.
        let long = "the quick brown fox jumps over the lazy dog ".repeat(50);
        let once = truncate_tail(&long, 200).into_owned();
        let twice = truncate_tail(&once, 200);
        assert_eq!(once, twice.into_owned());
    }
}
