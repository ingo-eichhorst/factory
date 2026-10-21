//! `factory memory` and `factory skill` (`#339`): a scope's memory and the
//! skills its runs may improve. The writes come from inside a run and carry
//! its token, the way `task report` does; reading is open to everyone; and
//! accepting a change, curating and restoring are the owner's.
use crate::client::Client;
use crate::{need_id, print};
use anyhow::{anyhow, Context, Result};
use clap::Subcommand;
use factory_core::protocol::{
    ArchiveRecord, CurationReport, MemoryOp, MemoryReceipt, MemoryTarget, MemoryWrite, Payload, Proposal,
    ProposalState, Request, ScopeMemory, SkillRow, SkillSubmission, SubmittedFile,
};
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub(crate) enum MemoryCmd {
    /// A scope's facts, owner profile and skills, as its next run would be
    /// handed them.
    Show {
        /// Defaults to FACTORY_SCOPE.
        #[arg(long)]
        scope: Option<String>,
    },
    /// Remember one thing: a new entry at the end of this run's scope's
    /// facts (or, with --profile, the owner profile). Refused, with the
    /// entries to drop, when it would go over budget; refused outright when
    /// it looks like a secret. Journaled on the run.
    Add {
        /// The entry: one fact, as its own paragraph.
        text: String,
        #[command(flatten)]
        on: Target,
    },
    /// Rewrite one entry, named by --entry <n> or --match <text>.
    Replace {
        /// The entry as it should read.
        text: String,
        #[command(flatten)]
        on: Target,
        #[command(flatten)]
        which: Which,
    },
    /// Forget one entry, named by --entry <n> or --match <text>.
    Remove {
        #[command(flatten)]
        on: Target,
        #[command(flatten)]
        which: Which,
    },
}

/// Which run is writing, and to which file.
#[derive(clap::Args)]
pub(crate) struct Target {
    /// The task whose run this is. Defaults to FACTORY_TASK_ID.
    #[arg(long)]
    task: Option<String>,
    /// Write the owner profile instead of this scope's facts.
    #[arg(long)]
    profile: bool,
    #[arg(long = "run-token", env = "FACTORY_TASK_TOKEN")]
    token: Option<String>,
}

/// Which entry: its number in the list a run is handed, or text only it holds.
#[derive(clap::Args)]
pub(crate) struct Which {
    #[arg(long)]
    entry: Option<u32>,
    #[arg(long = "match")]
    matching: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum SkillCmd {
    /// Every declared skill: where it is, what a run may do with it, when a
    /// run was last handed it, and any change waiting on the owner.
    List {
        /// Only this scope and the scopes below it.
        #[arg(long)]
        scope: Option<String>,
    },
    /// From inside a run: a better procedure for one of this scope's skills.
    /// Copy the files you change into a directory of your own, keeping
    /// their paths relative to the skill, edit them there, and give that
    /// directory as --from. Only the files given change, and only those
    /// that differ; --remove deletes one. `learn: propose` files the change
    /// for the owner to accept in the Inbox, `learn: apply` writes it at
    /// once, `learn: off` refuses. Journaled on the run either way.
    Submit {
        /// The skill's name, as the run was handed it.
        skill: String,
        /// A directory holding the changed and added files.
        #[arg(long)]
        from: Option<PathBuf>,
        /// A file, relative to the skill, to delete (repeatable).
        #[arg(long)]
        remove: Vec<String>,
        /// Why the procedure is better this way, in one line.
        #[arg(long)]
        summary: String,
        /// Defaults to FACTORY_TASK_ID.
        #[arg(long)]
        task: Option<String>,
        #[arg(long = "run-token", env = "FACTORY_TASK_TOKEN")]
        token: Option<String>,
    },
    /// Proposed changes, newest first. Open ones unless --state says
    /// otherwise (open, accepted, rejected, applied or all).
    Proposals {
        #[arg(long)]
        scope: Option<String>,
        #[arg(long, default_value = "open")]
        state: String,
    },
    /// One proposal, with its diff.
    Show { id: String },
    /// Apply an open proposal to its skill. The owner's to decide.
    Accept { id: String },
    /// Turn an open proposal down, saying why.
    Reject {
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Archive, with a verified backup, every skill no run has been handed
    /// for --unused-weeks (the configured `daemon.skill_curation` number,
    /// else 12). --dry-run says what it would archive and changes nothing.
    Curate {
        #[arg(long = "unused-weeks")]
        unused_weeks: Option<u32>,
        #[arg(long = "dry-run")]
        dry_run: bool,
    },
    /// Every archived skill.
    Archived,
    /// Put the newest archive of a skill back where it was.
    Restore {
        name: String,
        /// When two scopes have archived a skill of that name.
        #[arg(long)]
        scope: Option<String>,
    },
}

fn write(op: MemoryOp, text: Option<String>, on: &Target, which: Option<&Which>) -> MemoryWrite {
    MemoryWrite {
        target: if on.profile { MemoryTarget::Profile } else { MemoryTarget::Facts },
        op,
        text,
        entry: which.and_then(|w| w.entry),
        matching: which.and_then(|w| w.matching.clone()),
        token: on.token.clone(),
    }
}

pub(crate) async fn memory_cmd(json: bool, client: &Client, cmd: MemoryCmd) -> Result<()> {
    let (task, write) = match cmd {
        MemoryCmd::Show { scope } => {
            let scope = scope
                .or_else(|| std::env::var("FACTORY_SCOPE").ok().filter(|s| !s.is_empty()))
                .ok_or_else(|| anyhow!("no --scope given and FACTORY_SCOPE is not set"))?;
            let payload = client.send(Request::MemoryShow { scope }).await?;
            return print(&payload, json, |p| match p {
                Payload::Memory { scope, memory } => Some(memory_text(scope, memory)),
                _ => None,
            });
        }
        MemoryCmd::Add { text, on } => (on.task.clone(), write(MemoryOp::Add, Some(text), &on, None)),
        MemoryCmd::Replace { text, on, which } => (on.task.clone(), write(MemoryOp::Replace, Some(text), &on, Some(&which))),
        MemoryCmd::Remove { on, which } => (on.task.clone(), write(MemoryOp::Remove, None, &on, Some(&which))),
    };
    let payload = client.send(Request::MemoryWrite { id: need_id(task)?, write }).await?;
    print(&payload, json, |p| match p {
        Payload::MemoryWritten { receipt } => Some(receipt_text(receipt)),
        _ => None,
    })
}

fn receipt_text(r: &MemoryReceipt) -> String {
    let file = match r.target {
        MemoryTarget::Facts => "facts",
        MemoryTarget::Profile => "owner profile",
    };
    let verb = match r.op {
        MemoryOp::Add => "added",
        MemoryOp::Replace => "replaced",
        MemoryOp::Remove => "removed",
    };
    format!(
        "{verb} [{}] in the {file}: {} entries, {} of {} characters ({})",
        r.entry, r.entries, r.chars, r.max_chars, r.path
    )
}

fn memory_text(scope: &str, memory: &ScopeMemory) -> String {
    if memory.is_empty() {
        return format!("scope {scope} keeps no memory and no skills");
    }
    let mut out = factory_core::adapter::agent::memory_section(memory);
    out.insert_str(0, &format!("scope {scope}\n"));
    out.trim_end().to_string()
}

/// Every regular file under `dir`, as a path relative to it and its text.
fn files_from(dir: &Path) -> Result<Vec<SubmittedFile>> {
    fn walk(base: &Path, at: &Path, out: &mut Vec<SubmittedFile>) -> Result<()> {
        let mut entries: Vec<_> = std::fs::read_dir(at)
            .with_context(|| format!("reading {}", at.display()))?
            .collect::<std::io::Result<_>>()
            .with_context(|| format!("reading {}", at.display()))?;
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                walk(base, &path, out)?;
            } else if kind.is_file() {
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("{} is not a text file; a skill change only carries text", path.display()))?;
                let rel = path.strip_prefix(base).unwrap_or(&path).display().to_string();
                out.push(SubmittedFile { path: rel, content });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    Ok(out)
}

pub(crate) async fn skill_cmd(json: bool, client: &Client, cmd: SkillCmd) -> Result<()> {
    match cmd {
        SkillCmd::List { scope } => {
            let payload = client.send(Request::Skills { scope }).await?;
            print(&payload, json, |p| match p {
                Payload::Skills { skills } => Some(skills_text(skills)),
                _ => None,
            })
        }
        SkillCmd::Submit { skill, from, remove, summary, task, token } => {
            let files = match &from {
                Some(dir) => files_from(dir)?,
                None => Vec::new(),
            };
            if files.is_empty() && remove.is_empty() {
                return Err(anyhow!("nothing to submit: give --from <dir> with the changed files, or --remove <path>"));
            }
            let submission = SkillSubmission { skill, summary, files, remove, token };
            let payload = client.send(Request::SkillSubmit { id: need_id(task)?, submission }).await?;
            print(&payload, json, |p| match p {
                Payload::SkillProposal { proposal } => Some(match proposal.state {
                    ProposalState::Applied => format!("applied to skill {} ({})", proposal.skill, files_of(proposal)),
                    _ => format!(
                        "filed as {} for the owner to accept in the Inbox ({})",
                        proposal.id,
                        files_of(proposal)
                    ),
                }),
                _ => None,
            })
        }
        SkillCmd::Proposals { scope, state } => {
            let state = match state.as_str() {
                "all" => None,
                other => Some(other.parse::<ProposalState>().map_err(|e| anyhow!("--state: {e}, or all"))?),
            };
            let payload = client.send(Request::SkillProposals { scope, state }).await?;
            print(&payload, json, |p| match p {
                Payload::SkillProposals { proposals } if proposals.is_empty() => Some("no skill proposals".into()),
                Payload::SkillProposals { proposals } => Some(
                    proposals
                        .iter()
                        .map(|p| format!(
                            "{}  {}  {}/{}  {} ({})  {}",
                            p.id,
                            p.state.as_str(),
                            p.scope,
                            p.skill,
                            p.filed_at.format("%Y-%m-%d %H:%M"),
                            files_of(p),
                            p.summary
                        ))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            })
        }
        SkillCmd::Show { id } => {
            let payload = client.send(Request::SkillProposal { id }).await?;
            print(&payload, json, |p| match p {
                Payload::SkillProposal { proposal } => Some(proposal_text(proposal)),
                _ => None,
            })
        }
        SkillCmd::Accept { id } => {
            let payload = client.send(Request::SkillAccept { id }).await?;
            print(&payload, json, |p| match p {
                Payload::SkillProposal { proposal } => Some(format!("accepted; skill {} is changed ({})", proposal.skill, files_of(proposal))),
                _ => None,
            })
        }
        SkillCmd::Reject { id, reason } => {
            let payload = client.send(Request::SkillReject { id, reason }).await?;
            print(&payload, json, |p| match p {
                Payload::SkillProposal { proposal } => Some(format!("rejected; skill {} is unchanged", proposal.skill)),
                _ => None,
            })
        }
        SkillCmd::Curate { unused_weeks, dry_run } => {
            let payload = client.send(Request::SkillCurate { unused_weeks, dry_run }).await?;
            print(&payload, json, |p| match p {
                Payload::SkillCuration { report } => Some(curation_text(report)),
                _ => None,
            })
        }
        SkillCmd::Archived => {
            let payload = client.send(Request::SkillArchives).await?;
            print(&payload, json, |p| match p {
                Payload::SkillArchives { archives } if archives.is_empty() => Some("no archived skills".into()),
                Payload::SkillArchives { archives } => Some(archives.iter().map(archive_line).collect::<Vec<_>>().join("\n")),
                _ => None,
            })
        }
        SkillCmd::Restore { name, scope } => {
            let payload = client.send(Request::SkillRestore { name, scope }).await?;
            print(&payload, json, |p| match p {
                Payload::SkillRestored { archive } => Some(format!(
                    "restored skill {} to {} ({} files, from {})",
                    archive.name,
                    archive.dir,
                    archive.files.len(),
                    archive.id
                )),
                _ => None,
            })
        }
    }
}

fn files_of(p: &Proposal) -> String {
    p.changes.iter().map(|c| c.path.as_str()).collect::<Vec<_>>().join(", ")
}

fn proposal_text(p: &Proposal) -> String {
    let mut out = format!(
        "{}  {}  skill {} in {}\nfiled {} by {} (task {}, run {})\n{}\n",
        p.id,
        p.state.as_str(),
        p.skill,
        p.scope,
        p.filed_at.format("%Y-%m-%d %H:%M UTC"),
        p.origin.agent,
        p.origin.task_id,
        p.origin.run_id,
        p.summary
    );
    if let (Some(at), Some(by)) = (p.decided_at, &p.decided_by) {
        out.push_str(&format!("decided {} by {by}", at.format("%Y-%m-%d %H:%M UTC")));
        if let Some(reason) = &p.reason {
            out.push_str(&format!(": {reason}"));
        }
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&p.diff);
    out.trim_end().to_string()
}

fn skills_text(skills: &[SkillRow]) -> String {
    if skills.is_empty() {
        return "no scope declares a skill".into();
    }
    skills
        .iter()
        .map(|s| {
            let state = match (&s.archived, s.present) {
                (Some(id), _) => format!("archived ({id})"),
                (None, true) => "present".into(),
                (None, false) => "missing".into(),
            };
            let used = s
                .last_used
                .map(|at| format!("last handed to a run {}", at.format("%Y-%m-%d")))
                .unwrap_or_else(|| "never handed to a run yet".into());
            let waiting = if s.open_proposals > 0 { format!(", {} change(s) waiting", s.open_proposals) } else { String::new() };
            format!("{}/{}  learn: {}  {state}  {used}{waiting}  {}", s.scope, s.name, s.learn, s.dir)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn archive_line(a: &ArchiveRecord) -> String {
    let restored = a.restored_at.map(|at| format!(", restored {}", at.format("%Y-%m-%d"))).unwrap_or_default();
    format!(
        "{}  {}/{}  archived {}{restored}  ({} files) -- {}",
        a.id,
        a.scope,
        a.name,
        a.archived_at.format("%Y-%m-%d"),
        a.files.len(),
        a.reason
    )
}

fn curation_text(r: &CurationReport) -> String {
    if r.unused.is_empty() {
        return format!("no skill has gone unused for {} weeks; {} kept", r.unused_weeks, r.kept);
    }
    let mut lines: Vec<String> = r
        .unused
        .iter()
        .map(|u| {
            let outcome = match (&u.archive, &u.error, r.dry_run) {
                (_, _, true) => "would be archived".to_string(),
                (Some(id), _, _) => format!("archived as {id}"),
                (None, Some(e), _) => format!("NOT archived: {e}"),
                (None, None, _) => "left alone".into(),
            };
            format!("{}/{}  unused since {}  {outcome}", u.scope, u.name, u.since.format("%Y-%m-%d"))
        })
        .collect();
    lines.push(format!("{} kept", r.kept));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(subcommand)]
        command: Top,
    }

    #[derive(clap::Subcommand)]
    enum Top {
        Memory {
            #[command(subcommand)]
            command: MemoryCmd,
        },
        Skill {
            #[command(subcommand)]
            command: SkillCmd,
        },
    }

    #[test]
    fn memory_writes_name_their_entry_and_file_and_carry_the_run_token() {
        let cli = Cli::try_parse_from(["factory", "memory", "replace", "--task", "t1", "--profile", "--run-token", "tok", "--entry", "2", "new text"]).unwrap();
        let Top::Memory { command: MemoryCmd::Replace { text, on, which } } = cli.command else { panic!("not replace") };
        let w = write(MemoryOp::Replace, Some(text), &on, Some(&which));
        assert_eq!((w.target, w.entry, w.token.as_deref(), on.task.as_deref()), (MemoryTarget::Profile, Some(2), Some("tok"), Some("t1")));
        assert_eq!(w.text.as_deref(), Some("new text"));
        let cli = Cli::try_parse_from(["factory", "memory", "remove", "--match", "DevLand"]).unwrap();
        let Top::Memory { command: MemoryCmd::Remove { on, which } } = cli.command else { panic!("not remove") };
        let w = write(MemoryOp::Remove, None, &on, Some(&which));
        assert_eq!((w.target, w.matching.as_deref(), w.text), (MemoryTarget::Facts, Some("DevLand"), None));
        assert!(Cli::try_parse_from(["factory", "memory", "add"]).is_err(), "add needs its text");
    }

    #[test]
    fn a_skill_submission_carries_every_file_under_from_relative_to_it() {
        let dir = std::env::temp_dir().join(format!("factory-cli-skill-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("references")).unwrap();
        std::fs::write(dir.join("SKILL.md"), "# steps\n").unwrap();
        std::fs::write(dir.join("references/api.md"), "GET /sessions\n").unwrap();
        let files = files_from(&dir).unwrap();
        assert_eq!(
            files.iter().map(|f| (f.path.as_str(), f.content.as_str())).collect::<Vec<_>>(),
            vec![("SKILL.md", "# steps\n"), ("references/api.md", "GET /sessions\n")]
        );
        let _ = std::fs::remove_dir_all(&dir);
        let cli = Cli::try_parse_from(["factory", "skill", "submit", "radar", "--from", "/tmp/x", "--summary", "why", "--remove", "old.md"]).unwrap();
        assert!(matches!(cli.command, Top::Skill { command: SkillCmd::Submit { ref skill, ref remove, .. } } if skill == "radar" && remove == &["old.md".to_string()]));
        assert!(Cli::try_parse_from(["factory", "skill", "submit", "radar"]).is_err(), "a submission needs its summary");
    }
}
