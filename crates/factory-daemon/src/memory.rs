//! `#339`: a scope's memory and skills, as L3 serves them.
//!
//! The domain is `factory_agents::memory` and `factory_agents::skills`; this
//! is the service around it: the one write lock, the configuration snapshot
//! each call reads its declarations from, the `Learning` port L4 commands
//! writes through, the reads and owner actions that need no run, and the
//! curation loop (`daemon.skill_curation`). Every file it touches is either
//! the scope's own (memory, skills) or daemon state under the instance root's
//! `.factory/skills/` (proposals, usage, archives) -- never a database row.
use crate::dispatch_port::L3Port;
use crate::engine::Engine;
use crate::l3_service::L3Service;
use chrono::{DateTime, Utc};
use factory_agents::memory::{MemoryReceipt, MemoryScope, MemoryWrite};
use factory_agents::skills::{
    self, ArchiveRecord, CurationReport, Origin, Proposal, ProposalState, ProposalStore, SkillRow, SkillSubmission,
    UsageLog,
};
use factory_kernel::{FactoryError, Learn, Result, ScopeMemory, SkillHandle};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Where Factory keeps what it knows about skills: `<root>/.factory/skills`.
pub(crate) fn state_dir(factory: &factory_core::config::Factory) -> PathBuf {
    factory.factory_dir().join("skills")
}

fn failed(e: impl std::fmt::Display) -> FactoryError {
    FactoryError::Other(anyhow::anyhow!("{e}"))
}

impl L3Service<'_> {
    fn learning_inputs(&self) -> (PathBuf, PathBuf, Vec<MemoryScope>, Vec<factory_core::secrets::SecretDecl>) {
        let factory = self.wiring.snapshot();
        (factory.root.clone(), state_dir(&factory), factory.config.memory_scopes(), factory.config.secrets.clone())
    }

    fn memory_scope(scopes: &[MemoryScope], scope: &str) -> Result<MemoryScope> {
        scopes
            .iter()
            .find(|s| s.name == scope)
            .cloned()
            .ok_or_else(|| FactoryError::BadRequest(format!("no such scope: {scope:?}")))
    }

    /// Run `f` on a blocking thread holding the write lock.
    async fn locked<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> std::result::Result<T, String> + Send + 'static,
    ) -> Result<T> {
        let lock = self.state.learning.clone();
        tokio::task::spawn_blocking(move || {
            let _held = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            f()
        })
        .await
        .map_err(failed)?
        .map_err(FactoryError::BadRequest)
    }

    /// `factory memory show`: a scope's memory and skills, read now.
    pub(crate) async fn scope_memory(&self, scope: &str) -> Result<(String, ScopeMemory)> {
        let factory = self.wiring.snapshot();
        let name = factory.scope(scope).map(|s| s.name.clone()).map_err(|_| FactoryError::BadRequest(format!("no such scope: {scope:?}")))?;
        let (root, state, scopes, _) = self.learning_inputs();
        let found = Self::memory_scope(&scopes, &name)?;
        let memory = tokio::task::spawn_blocking(move || factory_agents::memory::read(&root, &state, &found))
            .await
            .map_err(failed)?;
        Ok((name, memory))
    }

    /// `factory skill list`: every declared skill, in `scope`'s subtree when
    /// one is named.
    pub(crate) async fn skill_rows(&self, scope: Option<&str>) -> Result<Vec<SkillRow>> {
        let (root, state, scopes, _) = self.learning_inputs();
        let members = self.subtree(scope)?;
        let scopes: Vec<MemoryScope> = scopes.into_iter().filter(|s| members.as_ref().is_none_or(|m| m.contains(&s.name))).collect();
        tokio::task::spawn_blocking(move || {
            let proposals = ProposalStore::new(&state).all();
            skills::rows(&root, &state, &scopes, &proposals)
        })
        .await
        .map_err(failed)
    }

    /// The scope a name means and every scope below it, or `None` for all.
    fn subtree(&self, scope: Option<&str>) -> Result<Option<BTreeSet<String>>> {
        let Some(name) = scope else { return Ok(None) };
        let snapshot = self.wiring.snapshot();
        let (asked, below) = factory_core::config::subtree_scopes(&snapshot, Some(name))?;
        let asked = asked.ok_or_else(|| FactoryError::BadRequest(format!("no such scope: {name:?}")))?;
        let mut members: BTreeSet<String> = below.into_iter().map(|s| s.name).collect();
        members.insert(asked.name);
        Ok(Some(members))
    }

    /// `factory skill proposals` and the Inbox: newest first, without the
    /// new file texts (the diff is kept -- it is what a person decides on).
    pub(crate) async fn skill_proposals(&self, scope: Option<&str>, state: Option<ProposalState>) -> Result<Vec<Proposal>> {
        let (_, dir, _, _) = self.learning_inputs();
        let members = self.subtree(scope)?;
        let all = tokio::task::spawn_blocking(move || ProposalStore::new(&dir).all()).await.map_err(failed)?;
        Ok(all
            .into_iter()
            .filter(|p| state.is_none_or(|s| p.state == s) && members.as_ref().is_none_or(|m| m.contains(&p.scope)))
            .map(|p| p.listing())
            .collect())
    }

    pub(crate) async fn skill_proposal(&self, id: &str) -> Result<Proposal> {
        let (_, dir, _, _) = self.learning_inputs();
        let id = id.to_string();
        let found = tokio::task::spawn_blocking(move || ProposalStore::new(&dir).get(&id).map(|p| (id, p)))
            .await
            .map_err(failed)?
            .map_err(FactoryError::BadRequest)?;
        found.1.ok_or_else(|| FactoryError::BadRequest(format!("no such skill proposal: {:?}", found.0)))
    }

    /// `factory skill archived`: every archive, oldest first.
    pub(crate) async fn skill_archives(&self) -> Result<Vec<ArchiveRecord>> {
        let (_, dir, _, _) = self.learning_inputs();
        tokio::task::spawn_blocking(move || skills::archives(&dir)).await.map_err(failed)
    }

    /// One curation pass, now: `factory skill curate` and the schedule.
    pub(crate) async fn curate_skills(&self, unused_weeks: u32, dry_run: bool, now: DateTime<Utc>) -> Result<CurationReport> {
        let (root, state, scopes, _) = self.learning_inputs();
        let report = self
            .locked(move || {
                let open: BTreeSet<(String, String)> = ProposalStore::new(&state)
                    .all()
                    .into_iter()
                    .filter(|p| p.state == ProposalState::Open)
                    .map(|p| (p.scope, p.skill))
                    .collect();
                Ok(skills::curate(&root, &state, &scopes, &open, unused_weeks, now, dry_run))
            })
            .await?;
        for unused in &report.unused {
            match (&unused.archive, &unused.error) {
                (Some(id), _) => tracing::info!(scope = %unused.scope, skill = %unused.name, archive = %id, "skill archived: unused since {}", unused.since),
                (None, Some(e)) => tracing::warn!(scope = %unused.scope, skill = %unused.name, "skill not archived: {e}"),
                (None, None) => {}
            }
        }
        Ok(report)
    }

    /// `factory skill restore <name>`: the newest archive of that name, put
    /// back where it was. A scope disambiguates two of the same name.
    pub(crate) async fn restore_skill(&self, name: &str, scope: Option<&str>) -> Result<ArchiveRecord> {
        let (_, state, _, _) = self.learning_inputs();
        let scope = match scope {
            Some(scope) => Some(self.wiring.snapshot().canonical_scope_name(scope)),
            None => None,
        };
        let name = name.to_string();
        let record = self
            .locked(move || {
                let record = skills::archive_named(&state, &name, scope.as_deref())?;
                skills::restore(&state, &record.id, Utc::now())
            })
            .await?;
        tracing::info!(scope = %record.scope, skill = %record.name, archive = %record.id, "skill restored");
        Ok(record)
    }
}

#[async_trait::async_trait]
impl factory_agents::dispatch::Learning for L3Port<'_> {
    async fn write_memory(&self, scope: &str, write: &MemoryWrite) -> Result<MemoryReceipt> {
        let (root, _, scopes, catalogue) = self.0.learning_inputs();
        let found = L3Service::memory_scope(&scopes, scope)?;
        let write = write.clone();
        self.0.locked(move || factory_agents::memory::write(&root, &found, &write, &catalogue)).await
    }

    async fn submit_skill(&self, scope: &str, origin: Origin, submission: &SkillSubmission) -> Result<Proposal> {
        let (root, state, scopes, catalogue) = self.0.learning_inputs();
        let found = L3Service::memory_scope(&scopes, scope)?;
        let submission = submission.clone();
        let scope = scope.to_string();
        self.0
            .locked(move || {
                let Some((decl, dir)) = skills::find(&root, &state, &found, &submission.skill) else {
                    let names: Vec<String> = found
                        .skills
                        .iter()
                        .map(|d| skills::skill_name(&state, &scope, d, &found.dir(&root).join(d.path.trim_end_matches('/'))))
                        .collect();
                    return Err(if names.is_empty() {
                        format!("scope {scope:?} declares no skills")
                    } else {
                        format!("scope {scope:?} has no skill {:?}; its skills are {}", submission.skill, names.join(", "))
                    });
                };
                if decl.learn == Learn::Off {
                    return Err(format!(
                        "skill {:?} is `learn: off`: runs follow it and do not change it. Say what should change with \
                         `factory task suggest` instead",
                        submission.skill
                    ));
                }
                let id = uuid::Uuid::new_v4().to_string();
                let mut proposal = skills::prepare(&scope, decl, &submission.skill, &dir, &submission, origin, id, Utc::now(), &catalogue)?;
                if decl.learn == Learn::Apply {
                    skills::apply_changes(&proposal)?;
                    proposal.state = ProposalState::Applied;
                    proposal.decided_at = Some(proposal.filed_at);
                    proposal.decided_by = Some("learn: apply".into());
                }
                ProposalStore::new(&state).put(&proposal)?;
                Ok(proposal)
            })
            .await
    }

    async fn decide_skill(&self, id: &str, accept: bool, by: &str, reason: Option<String>) -> Result<Proposal> {
        let (_, state, _, _) = self.0.learning_inputs();
        let (id, by) = (id.to_string(), by.to_string());
        self.0
            .locked(move || {
                let store = ProposalStore::new(&state);
                let mut proposal = store.get(&id)?.ok_or_else(|| format!("no such skill proposal: {id:?}"))?;
                if proposal.state != ProposalState::Open {
                    return Err(format!("skill proposal {id} is already {}", proposal.state.as_str()));
                }
                if accept {
                    skills::apply_changes(&proposal)?;
                    proposal.state = ProposalState::Accepted;
                } else {
                    proposal.state = ProposalState::Rejected;
                    proposal.reason = reason.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
                }
                proposal.decided_at = Some(Utc::now());
                proposal.decided_by = Some(by);
                store.put(&proposal)?;
                Ok(proposal)
            })
            .await
    }

    async fn skills_handed(&self, scope: &str, handed: &[SkillHandle], run_id: &str) {
        let (root, state, scopes, _) = self.0.learning_inputs();
        let Ok(found) = L3Service::memory_scope(&scopes, scope) else { return };
        let (handed, run_id) = (handed.to_vec(), run_id.to_string());
        let recorded = self
            .0
            .locked(move || {
                let mut usage = UsageLog::load(&state);
                let now = Utc::now();
                for decl in &found.skills {
                    let dir = found.dir(&root).join(decl.path.trim_end_matches('/'));
                    if let Some(skill) = handed.iter().find(|s| PathBuf::from(&s.path) == dir) {
                        usage.used(&found.name, &decl.path, &skill.name, &run_id, now);
                    }
                }
                usage.save(&state)
            })
            .await;
        if let Err(e) = recorded {
            tracing::warn!(scope, "recording skill use: {e}");
        }
    }
}

/// How often the curation loop looks at its schedule -- backup's own tick.
const CURATION_TICK: std::time::Duration = std::time::Duration::from_secs(60);

/// `daemon.skill_curation`'s loop: once a minute, curate when the schedule
/// says a pass is due since the last one (or since the daemon booted, the
/// first time), so a daemon that was down catches up once rather than for
/// every slot it missed. Without the block it does nothing.
pub async fn run(engine: Arc<Engine>, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let mut ticker = tokio::time::interval(CURATION_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() { return; }
            }
        }
        tick(&engine, Utc::now()).await;
    }
}

async fn tick(engine: &Engine, now: DateTime<Utc>) {
    let factory = engine.factory_snapshot();
    let Some(config) = factory.config.daemon.skill_curation.clone() else { return };
    let state = state_dir(&factory);
    let since = tokio::task::spawn_blocking(move || skills::CurationStamp::load(&state).last_run_at)
        .await
        .ok()
        .flatten()
        .unwrap_or(engine.shared.booted_at);
    match crate::schedule::next_after(&config.schedule.as_schedule(), since) {
        Ok(due) if due <= now => {
            if let Err(e) = engine.l3_service().curate_skills(config.unused_weeks, false, now).await {
                tracing::warn!("skill curation: {e}");
            }
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("daemon.skill_curation.schedule: {e}"),
    }
}

#[cfg(test)]
mod tests;
