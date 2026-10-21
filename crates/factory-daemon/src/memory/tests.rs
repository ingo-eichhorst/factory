//! `#339` through the engine: a run writes its scope's memory and improves its
//! skills with its own token, the journal says what changed, the owner decides
//! proposals, and curation archives and restores -- every file in the scope or
//! under `.factory/skills/`, none in the database.
use crate::engine::Engine;
use factory_core::config::{Config, Factory, Scope};
use factory_core::protocol::{
    Envelope, MemoryOp, MemoryWrite, Payload, ProposalState, Request, Response, SkillSubmission, SubmittedFile,
};
use factory_core::run::{NewRun, Run, Trigger};
use factory_core::task::{NewTask, Task};
use factory_kernel::MemoryTarget;
use factory_plugins::{Registry, SqliteStore};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const SKILL: &str = "---\nname: conference-radar-tracking\ndescription: Track CFPs.\n---\n\n# Steps\n1. Read the tracker.\n2. Search.\n";

struct Rig {
    engine: Arc<Engine>,
    root: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Rig {
    fn scope_dir(&self) -> PathBuf {
        self.root.join("projects/radar")
    }
}

/// One scope, `radar`, at `projects/radar` under a fresh root, with a skill
/// directory and whatever `scope_block` declares; `daemon` and `secrets` go
/// into the root config as written.
fn rig(scope_block: &str, secrets: &str) -> Rig {
    let root = std::env::temp_dir().join(format!("factory-memory-engine-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("projects/radar/skill/references")).unwrap();
    std::fs::write(root.join("projects/radar/skill/SKILL.md"), SKILL).unwrap();
    let mut config: Config = serde_yaml_ng::from_str(&format!(
        "version: 1\ninstance: {{ id: test, name: test }}\ndaemon: {{ power_assertion: false }}\n{secrets}"
    ))
    .unwrap();
    let mut scope: Scope = serde_yaml_ng::from_str(&format!("id: radar-id\nname: radar\n{scope_block}")).unwrap();
    scope.path = "projects/radar".into();
    config.scopes = vec![scope];
    config.validate().unwrap();
    let factory = Factory { root: root.clone(), config };
    let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
    let engine = Arc::new(Engine::new(factory, Registry::with_builtins(), store, PathBuf::from("factory"), Vec::new()));
    Rig { engine, root }
}

const MEMORY: &str = "memory:\n  facts: { path: memory/MEMORY.md, max_chars: 120 }\n  profile: { path: memory/USER.md, max_chars: 80, shared: owner }\n";

async fn task_with_run(engine: &Engine, token: &str) -> (Task, Run) {
    let task = factory_process::store::task_from_new(
        NewTask { title: "track the radar".into(), ..Default::default() },
        "radar".into(),
        "shell".into(),
        "herdr".into(),
    );
    let task = engine.l4.store.create(&task).await.unwrap();
    let run = engine
        .l4.store
        .create_run(&NewRun {
            task_id: task.id.clone(),
            trigger: Trigger::Manual,
            agent: "shell".into(),
            adapter: "shell".into(),
            runtime: "herdr".into(),
            token: token.into(),
            queued_at: None,
            scheduled_for: None,
        })
        .await
        .unwrap();
    (task, run)
}

fn add(target: MemoryTarget, text: &str, token: Option<&str>) -> MemoryWrite {
    MemoryWrite { target, op: MemoryOp::Add, text: Some(text.into()), entry: None, matching: None, token: token.map(Into::into) }
}

fn error_of(response: Response) -> String {
    match response {
        Response::Error { message, .. } => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[tokio::test]
async fn a_run_writes_its_own_scopes_memory_with_its_token_within_budget_and_the_journal_says_what() {
    let rig = rig(MEMORY, "");
    let engine = &rig.engine;
    let (task, run) = task_with_run(engine, "tok").await;
    let facts = rig.scope_dir().join("memory/MEMORY.md");

    let wrong = engine.handle_request(Request::MemoryWrite { id: task.id.clone(), write: add(MemoryTarget::Facts, "x", Some("nope")) }).await;
    assert!(error_of(wrong).contains("wrong token"), "the run's own token, or nothing");
    assert_eq!(read(&facts), "");

    let written = engine
        .handle_request(Request::MemoryWrite { id: task.id.clone(), write: add(MemoryTarget::Facts, "Accepted at DevLand 2027.", Some("tok")) })
        .await;
    let Response::Ok { data: Payload::MemoryWritten { receipt } } = written else { panic!("{written:?}") };
    assert_eq!((receipt.entry, receipt.chars, receipt.max_chars), (1, 25, 120));
    assert_eq!(read(&facts), "Accepted at DevLand 2027.\n");

    let profile = engine
        .handle_request(Request::MemoryWrite { id: task.id.clone(), write: add(MemoryTarget::Profile, "Asks in German.", Some("tok")) })
        .await;
    assert!(matches!(profile, Response::Ok { .. }), "{profile:?}");
    assert_eq!(read(&rig.root.join("memory/USER.md")), "Asks in German.\n", "the shared owner profile is at the instance root");

    // Over budget: refused with what to drop, nothing cut, nothing written.
    let long = "y".repeat(100);
    let refusal = error_of(engine.handle_request(Request::MemoryWrite { id: task.id.clone(), write: add(MemoryTarget::Facts, &long, Some("tok")) }).await);
    assert!(refusal.contains("over budget, so nothing was written"), "{refusal}");
    assert!(refusal.contains("[1] 25 chars: Accepted at DevLand 2027."), "{refusal}");
    assert!(refusal.contains("Removing [1] (the oldest) would make room"), "{refusal}");
    assert_eq!(read(&facts), "Accepted at DevLand 2027.\n");

    // A secret: refused by what it looks like, never echoed.
    let secret = "deploy key ghp_0123456789abcdefghijABCDEFGHIJklmnop";
    let refusal = error_of(engine.handle_request(Request::MemoryWrite { id: task.id.clone(), write: add(MemoryTarget::Facts, secret, Some("tok")) }).await);
    assert!(refusal.contains("looks like a secret -- a GitHub token"), "{refusal}");

    let replace = MemoryWrite { op: MemoryOp::Replace, entry: Some(1), ..add(MemoryTarget::Facts, "Accepted at DevLand 2027 (Berlin).", Some("tok")) };
    assert!(matches!(engine.handle_request(Request::MemoryWrite { id: task.id.clone(), write: replace }).await, Response::Ok { .. }));

    let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
    let memory: Vec<_> = entries.iter().filter(|e| e.kind == "memory").collect();
    assert!(memory.iter().all(|e| e.run_id.as_deref() == Some(run.id.as_str())), "every write and refusal is on the run");
    assert!(memory.iter().any(|e| e.message.starts_with("added [1] to the facts: Accepted at DevLand 2027.")), "{memory:?}");
    assert!(memory.iter().any(|e| e.message.starts_with("added [1] to the owner profile")), "{memory:?}");
    assert!(memory.iter().any(|e| e.message.starts_with("add of the facts refused:") && e.message.contains("over budget")));
    assert!(memory.iter().any(|e| e.message.starts_with("replaced [1] of the facts")));
    let replaced = memory.iter().find(|e| e.message.starts_with("replaced")).unwrap();
    assert_eq!(replaced.data.as_ref().unwrap()["write"]["old"], "Accepted at DevLand 2027.");
    assert!(entries.iter().all(|e| !e.message.contains("ghp_0123") && !e.data.as_ref().is_some_and(|d| d.to_string().contains("ghp_0123"))), "no secret in the journal");

    // No run in progress: nothing to write from.
    engine.l4.store.update_run(&run.id, &factory_core::run::RunPatch { status: Some(factory_core::run::RunStatus::Done), ended_at: Some(chrono::Utc::now()), ..Default::default() }).await.unwrap();
    let refusal = error_of(engine.handle_request(Request::MemoryWrite { id: task.id.clone(), write: add(MemoryTarget::Facts, "later", Some("tok")) }).await);
    assert!(refusal.contains("has no run in progress"), "{refusal}");
}

#[tokio::test]
async fn a_scope_without_memory_is_told_so_and_an_agent_writes_with_its_run_token_alone() {
    let rig = rig("", "");
    let engine = &rig.engine;
    let (task, _) = task_with_run(engine, "tok-a").await;
    let refusal = error_of(engine.handle_request(Request::MemoryWrite { id: task.id.clone(), write: add(MemoryTarget::Facts, "x", Some("tok-a")) }).await);
    assert!(refusal.contains("declares no memory.facts"), "{refusal}");

    let rig = self::rig(MEMORY, "");
    let engine = &rig.engine;
    let (task, _) = task_with_run(engine, "tok-b").await;
    // As the agent itself: the run token is the envelope's too.
    let response = engine
        .handle(Envelope { request: Request::MemoryWrite { id: task.id.clone(), write: add(MemoryTarget::Facts, "from the agent", Some("tok-b")) }, token: Some("tok-b".into()) })
        .await;
    assert!(matches!(response, Response::Ok { .. }), "{response:?}");
    // Deciding a skill change, curating and restoring are the owner's.
    for request in [
        Request::SkillAccept { id: "p".into() },
        Request::SkillReject { id: "p".into(), reason: "no".into() },
        Request::SkillCurate { unused_weeks: None, dry_run: true },
        Request::SkillRestore { name: "x".into(), scope: None },
    ] {
        let refusal = error_of(engine.handle(Envelope { request, token: Some("tok-b".into()) }).await);
        assert!(refusal.to_lowercase().contains("owner"), "{refusal}");
    }
}

#[tokio::test]
async fn dispatch_hands_a_run_its_scopes_memory_and_skills_journals_them_and_marks_the_skill_used() {
    let rig = rig(&format!("{MEMORY}skills:\n  - path: skill/\n    learn: propose\n  - path: gone/\n"), "");
    let engine = &rig.engine;
    std::fs::create_dir_all(rig.scope_dir().join("memory")).unwrap();
    std::fs::write(rig.scope_dir().join("memory/MEMORY.md"), "one\n§\ntwo\n").unwrap();
    let (task, run) = task_with_run(engine, "tok").await;

    let memory = engine.l4_service().memory_handout(&task, &run.id).await.expect("the scope keeps memory");
    assert_eq!(memory.facts.as_ref().unwrap().entries, vec!["one".to_string(), "two".to_string()]);
    assert!(memory.profile.as_ref().unwrap().entries.is_empty() && memory.profile.as_ref().unwrap().shared);
    assert_eq!(memory.skills[0].name, "conference-radar-tracking");
    assert!(memory.skills[0].present());
    assert!(memory.skills[1].error.as_deref().is_some_and(|e| e.contains("is not there")));

    let entries = engine.l4.store.entries(&task.id, 20).await.unwrap();
    let entry = entries.iter().find(|e| e.kind == "memory").expect("journaled");
    assert_eq!(entry.run_id.as_deref(), Some(run.id.as_str()));
    assert!(entry.message.contains("facts 9 of 120 chars (2 entries)"), "{}", entry.message);
    assert!(entry.message.contains("skill conference-radar-tracking (propose)"), "{}", entry.message);
    assert!(entry.message.contains("skill gone missing"), "{}", entry.message);
    assert_eq!(entry.data.as_ref().unwrap()["memory"]["facts"]["entries"][1], "two", "the whole handout, so the prompt stays explainable");

    let Response::Ok { data: Payload::Skills { skills } } = engine.handle_request(Request::Skills { scope: None }).await else { panic!() };
    assert_eq!(skills[0].last_run.as_deref(), Some(run.id.as_str()), "handing a skill to a run is using it");

    let none = self::rig("", "");
    let (task, run) = task_with_run(&none.engine, "tok").await;
    assert!(none.engine.l4_service().memory_handout(&task, &run.id).await.is_none());
    assert!(none.engine.l4.store.entries(&task.id, 20).await.unwrap().iter().all(|e| e.kind != "memory"));
}

fn submission(token: &str, content: &str) -> SkillSubmission {
    SkillSubmission {
        skill: "conference-radar-tracking".into(),
        summary: "sessionize has a JSON API".into(),
        files: vec![SubmittedFile { path: "SKILL.md".into(), content: content.into() }],
        remove: Vec::new(),
        token: Some(token.into()),
    }
}

#[tokio::test]
async fn a_proposed_skill_change_waits_for_the_owner_who_accepts_or_rejects_it_and_the_run_hears_either() {
    let rig = rig("skills:\n  - path: skill/\n    learn: propose\n", "");
    let engine = &rig.engine;
    let (task, run) = task_with_run(engine, "tok").await;
    let skill = rig.scope_dir().join("skill/SKILL.md");
    let better = SKILL.replace("2. Search.", "2. Search; sessionize has a JSON API.");

    let filed = engine.handle_request(Request::SkillSubmit { id: task.id.clone(), submission: submission("tok", &better) }).await;
    let Response::Ok { data: Payload::SkillProposal { proposal } } = filed else { panic!("{filed:?}") };
    assert_eq!(proposal.state, ProposalState::Open);
    assert_eq!(read(&skill), SKILL, "propose changes nothing until the owner says so");
    assert!(rig.root.join(".factory/skills/proposals").join(format!("{}.json", proposal.id)).is_file());

    let Response::Ok { data: Payload::SkillProposals { proposals } } =
        engine.handle_request(Request::SkillProposals { scope: None, state: Some(ProposalState::Open) }).await
    else {
        panic!()
    };
    assert_eq!(proposals.len(), 1, "what the Inbox lists");
    assert!(proposals[0].diff.contains("+2. Search; sessionize has a JSON API."), "{}", proposals[0].diff);
    assert!(proposals[0].changes[0].content.is_none(), "a listing carries the diff, not the files");

    assert!(error_of(engine.handle_request(Request::SkillReject { id: proposal.id.clone(), reason: " ".into() }).await).contains("needs a reason"));
    let accepted = engine.handle_request(Request::SkillAccept { id: proposal.id.clone() }).await;
    assert!(matches!(accepted, Response::Ok { data: Payload::SkillProposal { ref proposal } } if proposal.state == ProposalState::Accepted), "{accepted:?}");
    assert_eq!(read(&skill), better);
    assert!(error_of(engine.handle_request(Request::SkillAccept { id: proposal.id.clone() }).await).contains("already accepted"));

    // A second change, filed against the skill as it was before the first was
    // accepted, no longer applies -- and is rejected instead, with a reason.
    let stale = engine.handle_request(Request::SkillSubmit { id: task.id.clone(), submission: submission("tok", "# rewritten\n") }).await;
    let Response::Ok { data: Payload::SkillProposal { proposal: second } } = stale else { panic!("{stale:?}") };
    std::fs::write(&skill, "# edited by hand meanwhile\n").unwrap();
    assert!(error_of(engine.handle_request(Request::SkillAccept { id: second.id.clone() }).await).contains("changed since this proposal was filed"));
    assert!(matches!(engine.handle_request(Request::SkillReject { id: second.id.clone(), reason: "superseded".into() }).await, Response::Ok { .. }));

    let entries = engine.l4.store.entries(&task.id, 50).await.unwrap();
    let skill_entries: Vec<_> = entries.iter().filter(|e| e.kind == "skill").collect();
    assert!(skill_entries.iter().all(|e| e.run_id.as_deref() == Some(run.id.as_str())));
    assert!(skill_entries.iter().any(|e| e.message.contains("waiting in the Inbox as")));
    assert!(skill_entries.iter().any(|e| e.message.starts_with("the owner accepted the change to skill conference-radar-tracking")));
    assert!(skill_entries.iter().any(|e| e.message.starts_with("the owner rejected") && e.message.ends_with("superseded")));
}

#[tokio::test]
async fn learn_apply_writes_at_once_and_learn_off_refuses() {
    let rig = rig("skills:\n  - path: skill/\n    learn: apply\n", "");
    let (task, _) = task_with_run(&rig.engine, "tok").await;
    let applied = rig.engine.handle_request(Request::SkillSubmit { id: task.id.clone(), submission: submission("tok", "# better\n") }).await;
    assert!(matches!(applied, Response::Ok { data: Payload::SkillProposal { ref proposal } } if proposal.state == ProposalState::Applied), "{applied:?}");
    assert_eq!(read(&rig.scope_dir().join("skill/SKILL.md")), "# better\n");
    let journal = rig.engine.l4.store.entries(&task.id, 20).await.unwrap();
    assert!(journal.iter().any(|e| e.kind == "skill" && e.message.starts_with("changed skill conference-radar-tracking (SKILL.md)")));

    let rig = self::rig("skills:\n  - path: skill/\n    learn: off\n", "");
    let (task, _) = task_with_run(&rig.engine, "tok").await;
    let refusal = error_of(rig.engine.handle_request(Request::SkillSubmit { id: task.id.clone(), submission: submission("tok", "# better\n") }).await);
    assert!(refusal.contains("is `learn: off`"), "{refusal}");
    let refusal = error_of(rig.engine.handle_request(Request::SkillSubmit { id: task.id, submission: SkillSubmission { skill: "nope".into(), ..submission("tok", "x") } }).await);
    assert!(refusal.contains("has no skill \"nope\"; its skills are conference-radar-tracking"), "{refusal}");
}

#[tokio::test]
async fn curation_archives_an_unused_skill_with_a_backup_and_restore_by_name_brings_it_back() {
    let rig = rig("skills:\n  - path: skill/\n", "");
    let engine = &rig.engine;
    let dir = rig.scope_dir().join("skill");

    let dry = engine.handle_request(Request::SkillCurate { unused_weeks: Some(0), dry_run: true }).await;
    let Response::Ok { data: Payload::SkillCuration { report } } = dry else { panic!("{dry:?}") };
    assert_eq!(report.unused.len(), 1);
    assert!(dir.is_dir(), "a dry run changes nothing");

    let pass = engine.handle_request(Request::SkillCurate { unused_weeks: Some(0), dry_run: false }).await;
    let Response::Ok { data: Payload::SkillCuration { report } } = pass else { panic!("{pass:?}") };
    let id = report.unused[0].archive.clone().unwrap_or_else(|| panic!("{:?}", report.unused[0].error));
    assert!(!dir.exists());
    let archive = rig.root.join(".factory/skills/archive").join(&id);
    assert!(archive.join("manifest.json").is_file() && archive.join("skill.tar.zst").is_file());

    let Response::Ok { data: Payload::Memory { memory, .. } } = engine.handle_request(Request::MemoryShow { scope: "radar".into() }).await else { panic!() };
    assert_eq!(memory.skills[0].name, "conference-radar-tracking", "still named what it was");
    assert_eq!(memory.skills[0].archived.as_deref(), Some(id.as_str()));
    let Response::Ok { data: Payload::SkillArchives { archives } } = engine.handle_request(Request::SkillArchives).await else { panic!() };
    assert_eq!(archives.len(), 1);

    let restored = engine.handle_request(Request::SkillRestore { name: "conference-radar-tracking".into(), scope: None }).await;
    assert!(matches!(restored, Response::Ok { data: Payload::SkillRestored { .. } }), "{restored:?}");
    assert_eq!(read(&dir.join("SKILL.md")), SKILL);
    let again = error_of(engine.handle_request(Request::SkillRestore { name: "conference-radar-tracking".into(), scope: None }).await);
    assert!(again.contains("no archived skill"), "{again}");
}

#[tokio::test]
async fn the_curation_loop_runs_only_when_its_schedule_is_due() {
    let rig = rig("skills:\n  - path: skill/\n", "");
    let dir = rig.scope_dir().join("skill");
    // Without the block: nothing, ever.
    super::tick(&rig.engine, chrono::Utc::now() + chrono::Duration::weeks(52)).await;
    assert!(dir.is_dir());
    rig.engine.shared.factory.write().unwrap().config.daemon.skill_curation =
        Some(serde_yaml_ng::from_str("schedule: { cron: \"0 4 * * 1\" }\nunused_weeks: 2\n").unwrap());
    // Due, but the skill was first seen just now: kept, and the pass recorded.
    let now = chrono::Utc::now() + chrono::Duration::weeks(1);
    super::tick(&rig.engine, now).await;
    assert!(dir.is_dir());
    let stamp = factory_agents::skills::CurationStamp::load(&rig.root.join(".factory/skills"));
    assert_eq!(stamp.last_run_at, Some(now));
    // Three weeks of nobody handing it to a run, at the next due slot: archived.
    super::tick(&rig.engine, now + chrono::Duration::weeks(3)).await;
    assert!(!dir.exists(), "unused for longer than unused_weeks");
}

#[test]
fn memory_belongs_under_scope_and_every_shared_profile_is_the_same_one() {
    let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str("version: 1\nscope: { id: a, name: a }\nmemory: { facts: { path: M.md, max_chars: 9 } }\n").unwrap();
    let error = factory_core::config::refuse_misplaced_memory(&doc, Path::new("/co/a/.factory/config.yaml")).unwrap_err().to_string();
    assert!(error.contains("top-level `memory:` block") && error.contains("scope.memory"), "{error}");
    let doc: serde_yaml_ng::Value = serde_yaml_ng::from_str("scope: { id: a, name: a, skills: [{ path: s }] }\n").unwrap();
    assert!(factory_core::config::refuse_misplaced_memory(&doc, Path::new("x")).is_ok());

    let mut config: Config = serde_yaml_ng::from_str("version: 1\ninstance: { id: t, name: t }\n").unwrap();
    let scope = |name: &str, path: &str| -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(&format!(
            "id: {name}\nname: {name}\nmemory:\n  profile: {{ path: {path}, max_chars: 100, shared: owner }}\n"
        ))
        .unwrap();
        scope.path = name.into();
        scope
    };
    config.scopes = vec![scope("a", "memory/USER.md"), scope("b", "memory/USER.md")];
    assert!(config.validate().is_ok());
    config.scopes.push(scope("c", "memory/OWNER.md"));
    let error = config.validate().unwrap_err().to_string();
    assert!(error.contains("both share the owner profile but disagree"), "{error}");

    let bad: std::result::Result<Scope, _> = serde_yaml_ng::from_str("id: a\nname: a\nskills: [{ path: s, learn: maybe }]\n");
    assert!(bad.is_err(), "learn is propose, apply or off");
    let bad: std::result::Result<Scope, _> = serde_yaml_ng::from_str("id: a\nname: a\nmemory: { facts: { path: M.md, max_chars: 9, budget: 3 } }\n");
    assert!(bad.is_err(), "a misspelt key is refused, not ignored");
}
