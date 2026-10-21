//! `#339`: what a run asks of its scope's memory and skills, as L4 serves it.
//!
//! L3 owns the files (`memory.rs`); this is the Process side of the edge. A
//! write comes from a run, so it is checked here the way `task report` is --
//! the run must be in progress and present its own token -- and its scope is
//! the run's task's, never one the caller names. The write itself goes down
//! through L3's `Learning` port, one command per kind, and what L3 says it
//! did is journaled on the run. At dispatch, the same scope's memory is read
//! as L3's `ScopeMemory` fact, handed to the run and journaled too.
use crate::l4_service::L4Service;
use factory_agents::dispatch::Learning;
use factory_agents::memory::{MemoryReceipt, MemoryWrite};
use factory_agents::skills::{Origin, Proposal, ProposalState, SkillSubmission};
use factory_core::run::Run;
use factory_core::task::{Task, TaskEntry};
use factory_kernel::{FactoryError, Result, ScopeMemory};

impl L4Service<'_> {
    /// The run a memory or skill write is made on: in progress, and the
    /// caller holding its token.
    async fn learning_run(&self, task_id: &str, token: Option<&str>, what: &str) -> Result<(Run, Task)> {
        let run = self.state.store.active_run(task_id).await?.ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "task {task_id} has no run in progress; {what} is changed only by a run, from inside it"
            ))
        })?;
        self.check_run_token(&run, token, task_id)?;
        let task = self.require(task_id).await?;
        Ok((run, task))
    }

    fn canonical_scope(&self, task: &Task) -> String {
        self.wiring.snapshot().canonical_scope_name(&task.scope)
    }

    /// `factory memory add|replace|remove`: one entry of this run's scope's
    /// facts or of the owner profile, journaled on the run with what it was
    /// and what it is now.
    pub(crate) async fn memory_write(&self, task_id: &str, write: MemoryWrite) -> Result<MemoryReceipt> {
        let (run, task) = self.learning_run(task_id, write.token.as_deref(), "memory").await?;
        let scope = self.canonical_scope(&task);
        let receipt = match self.wiring.l3().port().write_memory(&scope, &write).await {
            Ok(receipt) => receipt,
            Err(e) => {
                // A refusal is part of the record too: the budget held, or a
                // secret was kept out. Its text never repeats one.
                let first = e.to_string().lines().next().unwrap_or_default().to_string();
                self.entry(
                    task_id,
                    TaskEntry::new("agent", "memory", format!("{} of the {} refused: {first}", write.op.as_str(), write.target))
                        .in_run(&run.id),
                )
                .await;
                return Err(e);
            }
        };
        let file = match receipt.target {
            factory_kernel::MemoryTarget::Facts => "facts",
            factory_kernel::MemoryTarget::Profile => "owner profile",
        };
        let what = match (&receipt.old, &receipt.new) {
            (None, Some(new)) => format!("added [{}] to the {file}: {}", receipt.entry, factory_agents::memory::preview(new)),
            (Some(_), Some(new)) => format!("replaced [{}] of the {file}: {}", receipt.entry, factory_agents::memory::preview(new)),
            (Some(old), None) => format!("removed [{}] from the {file}: {}", receipt.entry, factory_agents::memory::preview(old)),
            (None, None) => format!("changed the {file}"),
        };
        self.entry(
            task_id,
            TaskEntry::new("agent", "memory", format!("{what} ({} of {} characters)", receipt.chars, receipt.max_chars))
                .in_run(&run.id)
                .with_data(serde_json::json!({ "scope": scope, "write": receipt })),
        )
        .await;
        Ok(receipt)
    }

    /// `factory skill submit`: this run's better procedure for one of its
    /// scope's skills, filed or written by the skill's `learn:`, and journaled
    /// on the run either way.
    pub(crate) async fn skill_submit(&self, task_id: &str, submission: SkillSubmission) -> Result<Proposal> {
        let (run, task) = self.learning_run(task_id, submission.token.as_deref(), "a skill").await?;
        let scope = self.canonical_scope(&task);
        let origin = Origin { task_id: task.id.clone(), run_id: run.id.clone(), agent: run.agent.clone() };
        let proposal = self.wiring.l3().port().submit_skill(&scope, origin, &submission).await?;
        let files = proposal.changes.iter().map(|c| c.path.as_str()).collect::<Vec<_>>().join(", ");
        let message = match proposal.state {
            ProposalState::Applied => format!("changed skill {} ({files}): {}", proposal.skill, proposal.summary),
            _ => format!(
                "proposed a change to skill {} ({files}), waiting in the Inbox as {}: {}",
                proposal.skill, proposal.id, proposal.summary
            ),
        };
        self.entry(
            task_id,
            TaskEntry::new("agent", "skill", message)
                .in_run(&run.id)
                .with_data(serde_json::json!({ "proposal": proposal.listing() })),
        )
        .await;
        Ok(proposal)
    }

    /// A person's answer to an open proposal, journaled on the run that
    /// filed it -- which is long over, so the entry says who decided.
    pub(crate) async fn skill_decide(&self, id: &str, accept: bool, by: &str, reason: Option<String>) -> Result<Proposal> {
        if !accept && reason.as_deref().is_none_or(|r| r.trim().is_empty()) {
            return Err(FactoryError::BadRequest("rejecting a skill change needs a reason, for the run that filed it".into()));
        }
        let proposal = self.wiring.l3().port().decide_skill(id, accept, by, reason).await?;
        let message = if accept {
            format!("{by} accepted the change to skill {} ({}); it is applied", proposal.skill, proposal.id)
        } else {
            format!(
                "{by} rejected the change to skill {} ({}): {}",
                proposal.skill,
                proposal.id,
                proposal.reason.as_deref().unwrap_or_default()
            )
        };
        self.entry(
            &proposal.origin.task_id,
            TaskEntry::new("daemon", "skill", message)
                .in_run(&proposal.origin.run_id)
                .with_data(serde_json::json!({ "proposal": proposal.listing() })),
        )
        .await;
        Ok(proposal)
    }

    /// The memory and skills a run of `task` is handed (`#339`): read once,
    /// now, from the scope's files, journaled whole so the prompt can still
    /// be explained after the files move on, and each skill marked as used.
    /// `None` for a scope that keeps none. A read that fails is journaled and
    /// the run starts without -- memory is a help, never a reason not to.
    pub(crate) async fn memory_handout(&self, task: &Task, run_id: &str) -> Option<ScopeMemory> {
        let scope = self.canonical_scope(task);
        let memory = match self.wiring.facts().get::<ScopeMemory>(&scope).await {
            Ok(memory) if memory.is_empty() => return None,
            Ok(memory) => memory,
            Err(e) => {
                self.entry(
                    &task.id,
                    TaskEntry::new("daemon", "memory", format!("reading this scope's memory failed, so this run has none: {e}"))
                        .in_run(run_id),
                )
                .await;
                return None;
            }
        };
        let mut parts = Vec::new();
        for (label, file) in [("facts", &memory.facts), ("owner profile", &memory.profile)] {
            if let Some(file) = file {
                parts.push(match &file.error {
                    Some(error) => format!("{label} unreadable ({error})"),
                    None => format!("{label} {} of {} chars ({} entries)", file.chars, file.max_chars, file.entries.len()),
                });
            }
        }
        for skill in &memory.skills {
            parts.push(match (&skill.archived, &skill.error) {
                (Some(id), _) => format!("skill {} archived ({id})", skill.name),
                (None, Some(_)) => format!("skill {} missing", skill.name),
                (None, None) => format!("skill {} ({})", skill.name, skill.learn),
            });
        }
        self.entry(
            &task.id,
            TaskEntry::new("daemon", "memory", format!("handed this scope's memory: {}", parts.join(", ")))
                .in_run(run_id)
                .with_data(serde_json::json!({ "scope": scope, "memory": memory })),
        )
        .await;
        let handed: Vec<_> = memory.skills.iter().filter(|s| s.present()).cloned().collect();
        if !handed.is_empty() {
            self.wiring.l3().port().skills_handed(&scope, &handed, run_id).await;
        }
        Some(memory)
    }
}
