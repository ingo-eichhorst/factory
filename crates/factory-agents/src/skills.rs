//! A scope's skills (`#339`): procedures a run is handed and may improve.
//!
//! ```yaml
//! scope:
//!   skills:
//!     - path: skill/          # SKILL.md + references/ + templates/
//!       learn: propose        # propose | apply | off
//! ```
//!
//! A skill is a directory in the scope. Every run of the scope is handed its
//! path; a run that finds a better procedure submits the changed files, and
//! what happens then is the skill's `learn:` -- `propose` files the change,
//! as a diff, for a person to accept in the Inbox; `apply` writes it at once;
//! `off` refuses. Either way the run's journal says so.
//!
//! **What Factory keeps, and where.** The skill files are the scope's own and
//! stay in its directory. Everything else is daemon state under the instance
//! root's `.factory/skills/`, never in the database and never in a scope:
//!
//! - `proposals/<id>.json` -- every submitted change, whole: the changed
//!   files' new text, a hash of each file as it was when the change was
//!   filed, and the rendered diff. A proposal is applied only to the files it
//!   was made against; if one changed since, accepting it is refused.
//! - `usage.json` -- when each skill was first seen and last handed to a run,
//!   which is what curation measures "unused" by.
//! - `archive/<scope>/<name>-<stamp>/` -- a skill curation archived:
//!   `manifest.json` (what it was, every file's size and hash) and
//!   `skill.tar.zst` (the files), verified against each other before the
//!   directory is removed, and again before `restore` puts it back.
use crate::memory::{relative_inside, write_atomically, MemoryScope, SkillDecl};
use chrono::{DateTime, Utc};
use factory_environment::secrets::SecretDecl;
use factory_kernel::Learn;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

/// One file of a submission may be at most this large, and the whole of it
/// this many times that: a skill is a procedure, not a dataset.
pub const MAX_FILE_BYTES: usize = 256 * 1024;
pub const MAX_SUBMISSION_BYTES: usize = 4 * MAX_FILE_BYTES;
pub const MAX_SUBMISSION_FILES: usize = 200;

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// ------------------------------------------------------------------ names

/// The `name:` in a `SKILL.md`'s YAML frontmatter, if it has one.
pub fn frontmatter_name(text: &str) -> Option<String> {
    let mut lines = text.lines();
    if lines.next()?.trim_end() != "---" {
        return None;
    }
    for line in lines {
        if line.trim_end() == "---" {
            return None;
        }
        if let Some(value) = line.strip_prefix("name:") {
            let value = value.trim().trim_matches(['"', '\'']).trim();
            return crate::memory::is_skill_name(value).then(|| value.to_string());
        }
    }
    None
}

/// A skill's name: as declared, else as its `SKILL.md` says, else its
/// directory's own name.
pub fn name_of(decl: &SkillDecl, dir: &Path) -> String {
    if let Some(name) = &decl.name {
        return name.clone();
    }
    std::fs::read_to_string(dir.join("SKILL.md"))
        .ok()
        .and_then(|text| frontmatter_name(&text))
        .unwrap_or_else(|| {
            Path::new(decl.path.trim_end_matches('/'))
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| decl.path.clone())
        })
}

/// The same declared path written two ways (`skill/`, `./skill`) is one.
fn same_path(a: &str, b: &str) -> bool {
    match (relative_inside(a), relative_inside(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// A declared skill's name wherever it is: an archived one is still called
/// what it was called when curation archived it, not after its empty path.
pub fn skill_name(state_dir: &Path, scope: &str, decl: &SkillDecl, dir: &Path) -> String {
    if decl.name.is_none() && !dir.is_dir() {
        if let Some(record) = latest_archive(state_dir, scope, &decl.path) {
            return record.name;
        }
    }
    name_of(decl, dir)
}

/// A scope's declared skill by name, with its directory.
pub fn find<'a>(root: &Path, state_dir: &Path, scope: &'a MemoryScope, name: &str) -> Option<(&'a SkillDecl, PathBuf)> {
    scope.skills.iter().find_map(|decl| {
        let dir = scope.dir(root).join(decl.path.trim_end_matches('/'));
        (skill_name(state_dir, &scope.name, decl, &dir) == name).then_some((decl, dir))
    })
}

// ------------------------------------------------------------------- diff

/// A unified diff of one file, `None` meaning absent. Line-based, three
/// lines of context, the way `diff -u` writes it.
pub fn unified_diff(path: &str, old: Option<&str>, new: Option<&str>) -> String {
    let a: Vec<&str> = old.map(|t| t.split_inclusive('\n').collect()).unwrap_or_default();
    let b: Vec<&str> = new.map(|t| t.split_inclusive('\n').collect()).unwrap_or_default();
    let ops = line_ops(&a, &b);
    let mut out = format!(
        "--- {}\n+++ {}\n",
        if old.is_some() { format!("a/{path}") } else { "/dev/null".into() },
        if new.is_some() { format!("b/{path}") } else { "/dev/null".into() },
    );
    const CONTEXT: usize = 3;
    // Group the edit script into hunks: runs of changes with up to
    // 2 * CONTEXT unchanged lines between them stay one hunk.
    let changed: Vec<usize> = ops.iter().enumerate().filter(|(_, op)| !matches!(op, Op::Same(..))).map(|(i, _)| i).collect();
    let mut hunks: Vec<(usize, usize)> = Vec::new();
    for i in changed {
        let start = i.saturating_sub(CONTEXT);
        let end = (i + CONTEXT + 1).min(ops.len());
        match hunks.last_mut() {
            Some((_, last_end)) if start <= *last_end => *last_end = end,
            _ => hunks.push((start, end)),
        }
    }
    for (start, end) in hunks {
        let (mut a_line, mut b_line) = (0, 0);
        for op in &ops[..start] {
            match op {
                Op::Same(..) => { a_line += 1; b_line += 1; }
                Op::Delete(_) => a_line += 1,
                Op::Insert(_) => b_line += 1,
            }
        }
        let a_count = ops[start..end].iter().filter(|op| !matches!(op, Op::Insert(_))).count();
        let b_count = ops[start..end].iter().filter(|op| !matches!(op, Op::Delete(_))).count();
        let range = |line: usize, count: usize| match count {
            0 => format!("{line},0"),
            1 => format!("{}", line + 1),
            n => format!("{},{n}", line + 1),
        };
        out.push_str(&format!("@@ -{} +{} @@\n", range(a_line, a_count), range(b_line, b_count)));
        for op in &ops[start..end] {
            let (mark, line) = match op {
                Op::Same(i, _) => (' ', a[*i]),
                Op::Delete(i) => ('-', a[*i]),
                Op::Insert(j) => ('+', b[*j]),
            };
            out.push(mark);
            out.push_str(line);
            if !line.ends_with('\n') {
                out.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Same(usize, usize),
    Delete(usize),
    Insert(usize),
}

/// An edit script from `a` to `b`: the common prefix and suffix as they
/// are, the middle by longest common subsequence -- or, past a size no
/// procedure reaches, as deleted and inserted whole.
fn line_ops(a: &[&str], b: &[&str]) -> Vec<Op> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..].iter().rev().zip(b[prefix..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let mut ops: Vec<Op> = (0..prefix).map(|i| Op::Same(i, i)).collect();
    if am.len().saturating_mul(bm.len()) <= 4_000_000 {
        // lcs[i][j]: the LCS length of am[i..] and bm[j..].
        let (n, m) = (am.len(), bm.len());
        let mut lcs = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                lcs[i * (m + 1) + j] = if am[i] == bm[j] {
                    lcs[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    lcs[(i + 1) * (m + 1) + j].max(lcs[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && am[i] == bm[j] {
                ops.push(Op::Same(prefix + i, prefix + j));
                i += 1;
                j += 1;
            } else if i < n && (j == m || lcs[(i + 1) * (m + 1) + j] >= lcs[i * (m + 1) + j + 1]) {
                // Deletions before insertions, as `diff -u` writes them.
                ops.push(Op::Delete(prefix + i));
                i += 1;
            } else {
                ops.push(Op::Insert(prefix + j));
                j += 1;
            }
        }
    } else {
        ops.extend((0..am.len()).map(|i| Op::Delete(prefix + i)));
        ops.extend((0..bm.len()).map(|j| Op::Insert(prefix + j)));
    }
    ops.extend((0..suffix).map(|k| Op::Same(a.len() - suffix + k, b.len() - suffix + k)));
    ops
}

// -------------------------------------------------------------- proposals

/// One file of a submission: its path relative to the skill directory and
/// its new text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmittedFile {
    pub path: String,
    pub content: String,
}

/// `factory skill submit`, as it travels to the daemon: the files a run
/// changed or added, and the ones it wants gone. The scope is never named:
/// it is always the run's own task's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillSubmission {
    pub skill: String,
    /// One line: why the procedure is better this way.
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<SubmittedFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove: Vec<String>,
    /// The run token, exactly as `task report` carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Added,
    Changed,
    Removed,
}

/// One file a proposal changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    /// The file's hash when the change was filed; `None` for one it adds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// The new text; `None` for a removal, and in a listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalState {
    /// Waiting for a person (`learn: propose`).
    Open,
    /// A person accepted it and it was applied.
    Accepted,
    /// A person turned it down.
    Rejected,
    /// Written at once, on submission (`learn: apply`).
    Applied,
}

impl ProposalState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Applied => "applied",
        }
    }
}

impl std::str::FromStr for ProposalState {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, String> {
        match s {
            "open" => Ok(Self::Open),
            "accepted" => Ok(Self::Accepted),
            "rejected" => Ok(Self::Rejected),
            "applied" => Ok(Self::Applied),
            other => Err(format!("{other:?} is not open, accepted, rejected or applied")),
        }
    }
}

/// The run a change came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub task_id: String,
    pub run_id: String,
    pub agent: String,
}

/// A change to a skill, filed by a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub id: String,
    pub scope: String,
    pub skill: String,
    /// The skill's declared path, relative to the scope.
    pub path: String,
    /// The skill's directory, absolute.
    pub dir: String,
    pub learn: Learn,
    pub summary: String,
    pub state: ProposalState,
    pub filed_at: DateTime<Utc>,
    pub origin: Origin,
    pub changes: Vec<FileChange>,
    /// Every change as one unified diff, paths relative to the skill.
    pub diff: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Proposal {
    /// The same proposal without the new file texts: what a listing shows.
    /// The diff is kept -- it is what a person decides on.
    pub fn listing(&self) -> Self {
        let mut out = self.clone();
        for change in &mut out.changes {
            change.content = None;
        }
        out
    }
}

/// Lines `new` has that `old` does not: what a change adds, and the only
/// text a secret check should look at -- an example key a skill already
/// carried is not this run's doing.
fn added_lines(old: Option<&str>, new: &str) -> String {
    let before: BTreeSet<&str> = old.map(|t| t.lines().collect()).unwrap_or_default();
    new.lines().filter(|line| !before.contains(line)).collect::<Vec<_>>().join("\n")
}

/// A path a submission names inside a skill, refused when it would leave it.
fn inside_skill(path: &str) -> std::result::Result<PathBuf, String> {
    relative_inside(path).map_err(|e| format!("{path:?} {e}"))
}

/// Turn a run's submission into a proposal against the skill as it is now,
/// touching nothing. Refused, with the reason, when it changes nothing,
/// escapes the skill, is too large, or holds something that looks like a
/// secret.
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    scope: &str,
    decl: &SkillDecl,
    name: &str,
    dir: &Path,
    submission: &SkillSubmission,
    origin: Origin,
    id: String,
    now: DateTime<Utc>,
    catalogue: &[SecretDecl],
) -> std::result::Result<Proposal, String> {
    let summary = submission.summary.trim();
    if summary.is_empty() {
        return Err("a skill change needs a --summary saying why the procedure is better this way".into());
    }
    if !dir.is_dir() {
        return Err(format!(
            "skill {name:?} has no directory at {} -- if curation archived it, `factory skill restore {name}` brings it back first",
            dir.display()
        ));
    }
    if submission.files.len() + submission.remove.len() > MAX_SUBMISSION_FILES {
        return Err(format!("a skill change touches at most {MAX_SUBMISSION_FILES} files"));
    }
    let total: usize = submission.files.iter().map(|f| f.content.len()).sum();
    if total > MAX_SUBMISSION_BYTES {
        return Err(format!("a skill change is at most {MAX_SUBMISSION_BYTES} bytes of text; this one is {total}"));
    }
    let mut seen = BTreeSet::new();
    let mut changes = Vec::new();
    let mut diff = String::new();
    for file in &submission.files {
        let rel = inside_skill(&file.path)?;
        if !seen.insert(rel.clone()) {
            return Err(format!("{} is named twice", rel.display()));
        }
        if file.content.len() > MAX_FILE_BYTES {
            return Err(format!("{} is {} bytes; one file of a skill is at most {MAX_FILE_BYTES}", rel.display(), file.content.len()));
        }
        let live = dir.join(&rel);
        let old = match std::fs::read(&live) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("reading {}: {e}", live.display())),
        };
        if old.as_deref() == Some(file.content.as_bytes()) {
            continue;
        }
        let old_text = match &old {
            Some(bytes) => Some(String::from_utf8(bytes.clone()).map_err(|_| {
                format!("{} is not text, and a skill change only replaces text files", rel.display())
            })?),
            None => None,
        };
        if let Some(finding) = factory_environment::secret_scan::find(&added_lines(old_text.as_deref(), &file.content), catalogue) {
            return Err(format!(
                "{} looks like it holds a secret -- {finding} -- so the change was not filed. A skill is handed to \
                 every later run: say where a credential is kept, never what it is",
                rel.display()
            ));
        }
        let path = rel.display().to_string();
        diff.push_str(&unified_diff(&path, old_text.as_deref(), Some(&file.content)));
        changes.push(FileChange {
            kind: if old.is_some() { ChangeKind::Changed } else { ChangeKind::Added },
            before: old.as_deref().map(sha256),
            content: Some(file.content.clone()),
            path,
        });
    }
    for path in &submission.remove {
        let rel = inside_skill(path)?;
        if !seen.insert(rel.clone()) {
            return Err(format!("{} is both changed and removed", rel.display()));
        }
        let live = dir.join(&rel);
        let bytes = std::fs::read(&live).map_err(|_| format!("{} is not in the skill, so it cannot be removed", rel.display()))?;
        let path = rel.display().to_string();
        diff.push_str(&unified_diff(&path, Some(&String::from_utf8_lossy(&bytes)), None));
        changes.push(FileChange { path, kind: ChangeKind::Removed, before: Some(sha256(&bytes)), content: None });
    }
    if changes.is_empty() {
        return Err(format!("nothing to change: every file given is identical to skill {name:?} as it is"));
    }
    Ok(Proposal {
        id,
        scope: scope.to_string(),
        skill: name.to_string(),
        path: decl.path.clone(),
        dir: dir.display().to_string(),
        learn: decl.learn,
        summary: summary.lines().next().unwrap_or_default().to_string(),
        state: ProposalState::Open,
        filed_at: now,
        origin,
        changes,
        diff,
        decided_at: None,
        decided_by: None,
        reason: None,
    })
}

/// Write a proposal's changes into the skill -- every file, or none: each
/// file is first checked to be exactly what the proposal was made against.
pub fn apply_changes(proposal: &Proposal) -> std::result::Result<(), String> {
    let dir = Path::new(&proposal.dir);
    for change in &proposal.changes {
        let live = dir.join(inside_skill(&change.path)?);
        let now = match std::fs::read(&live) {
            Ok(bytes) => Some(sha256(&bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("reading {}: {e}", live.display())),
        };
        if now != change.before {
            return Err(format!(
                "{} changed since this proposal was filed, so it can no longer be applied as it is -- reject it, \
                 and a later run can file the change again against the skill as it is now",
                change.path
            ));
        }
    }
    for change in &proposal.changes {
        let live = dir.join(inside_skill(&change.path)?);
        match (&change.kind, &change.content) {
            (ChangeKind::Removed, _) => std::fs::remove_file(&live).map_err(|e| format!("removing {}: {e}", live.display()))?,
            (_, Some(content)) => write_atomically(&live, content.as_bytes())?,
            (_, None) => return Err(format!("the proposal has no new text for {}", change.path)),
        }
    }
    Ok(())
}

/// Proposals, one JSON file each under `<state>/proposals/`.
#[derive(Debug, Clone)]
pub struct ProposalStore {
    pub dir: PathBuf,
}

fn is_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

impl ProposalStore {
    pub fn new(state_dir: &Path) -> Self {
        Self { dir: state_dir.join("proposals") }
    }

    pub fn put(&self, proposal: &Proposal) -> std::result::Result<(), String> {
        if !is_id(&proposal.id) {
            return Err(format!("{:?} is not a proposal id", proposal.id));
        }
        let json = serde_json::to_vec_pretty(proposal).map_err(|e| format!("encoding proposal: {e}"))?;
        write_atomically(&self.dir.join(format!("{}.json", proposal.id)), &json)
    }

    pub fn get(&self, id: &str) -> std::result::Result<Option<Proposal>, String> {
        if !is_id(id) {
            return Ok(None);
        }
        let path = self.dir.join(format!("{id}.json"));
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("reading {}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("reading {}: {e}", path.display())),
        }
    }

    /// Every proposal, newest first. One that cannot be read is skipped
    /// with a warning rather than hiding all the others.
    pub fn all(&self) -> Vec<Proposal> {
        let mut out: Vec<Proposal> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|e| e == "json"))
            .filter_map(|entry| match std::fs::read(entry.path()).map(|b| serde_json::from_slice::<Proposal>(&b)) {
                Ok(Ok(proposal)) => Some(proposal),
                Ok(Err(e)) => {
                    tracing::warn!(path = %entry.path().display(), "unreadable skill proposal: {e}");
                    None
                }
                Err(e) => {
                    tracing::warn!(path = %entry.path().display(), "unreadable skill proposal: {e}");
                    None
                }
            })
            .collect();
        out.sort_by(|a, b| b.filed_at.cmp(&a.filed_at).then_with(|| a.id.cmp(&b.id)));
        out
    }
}

// ------------------------------------------------------------------ usage

/// When one skill was first seen and last handed to a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillUse {
    pub scope: String,
    /// The declared path, as the config writes it.
    pub path: String,
    pub name: String,
    pub first_seen: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
}

impl SkillUse {
    /// What curation measures "unused" from.
    pub fn since(&self) -> DateTime<Utc> {
        self.last_used.unwrap_or(self.first_seen)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageLog {
    #[serde(default)]
    pub skills: Vec<SkillUse>,
}

impl UsageLog {
    fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("usage.json")
    }

    pub fn load(state_dir: &Path) -> Self {
        std::fs::read(Self::path(state_dir))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, state_dir: &Path) -> std::result::Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| format!("encoding skill usage: {e}"))?;
        write_atomically(&Self::path(state_dir), &json)
    }

    pub fn find(&self, scope: &str, path: &str) -> Option<&SkillUse> {
        self.skills.iter().find(|u| u.scope == scope && same_path(&u.path, path))
    }

    fn entry(&mut self, scope: &str, path: &str, name: &str, now: DateTime<Utc>) -> &mut SkillUse {
        let at = match self.skills.iter().position(|u| u.scope == scope && same_path(&u.path, path)) {
            Some(at) => at,
            None => {
                self.skills.push(SkillUse {
                    scope: scope.into(),
                    path: path.into(),
                    name: name.into(),
                    first_seen: now,
                    last_used: None,
                    last_run: None,
                });
                self.skills.len() - 1
            }
        };
        let entry = &mut self.skills[at];
        entry.name = name.into();
        entry
    }

    /// A run was handed this skill.
    pub fn used(&mut self, scope: &str, path: &str, name: &str, run_id: &str, now: DateTime<Utc>) {
        let entry = self.entry(scope, path, name, now);
        entry.last_used = Some(now);
        entry.last_run = Some(run_id.into());
    }

    /// Curation saw this skill: a skill never handed to a run is unused
    /// from the first time Factory saw it, never from the epoch.
    pub fn seen(&mut self, scope: &str, path: &str, name: &str, now: DateTime<Utc>) -> SkillUse {
        self.entry(scope, path, name, now).clone()
    }
}

// --------------------------------------------------------------- archives

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchivedFile {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// A skill curation archived: its `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveRecord {
    /// `<scope>/<name>-<stamp>`, its directory under `archive/`.
    pub id: String,
    pub name: String,
    pub scope: String,
    /// The declared path, relative to the scope.
    pub path: String,
    /// Where it was, absolute, and where `restore` puts it back.
    pub dir: String,
    pub archived_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<DateTime<Utc>>,
    pub reason: String,
    pub files: Vec<ArchivedFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restored_at: Option<DateTime<Utc>>,
}

const MANIFEST: &str = "manifest.json";
const BUNDLE: &str = "skill.tar.zst";

pub fn archive_root(state_dir: &Path) -> PathBuf {
    state_dir.join("archive")
}

/// A scope name as one directory component.
fn component(scope: &str) -> String {
    scope.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect()
}

/// Every archive, oldest first.
pub fn archives(state_dir: &Path) -> Vec<ArchiveRecord> {
    let mut out = Vec::new();
    for scope in std::fs::read_dir(archive_root(state_dir)).into_iter().flatten().flatten() {
        for entry in std::fs::read_dir(scope.path()).into_iter().flatten().flatten() {
            let manifest = entry.path().join(MANIFEST);
            if let Some(record) = std::fs::read(&manifest).ok().and_then(|b| serde_json::from_slice::<ArchiveRecord>(&b).ok()) {
                out.push(record);
            }
        }
    }
    out.sort_by(|a, b| a.archived_at.cmp(&b.archived_at).then_with(|| a.id.cmp(&b.id)));
    out
}

/// The newest archive of a scope's declared skill that has not been
/// restored since.
pub fn latest_archive(state_dir: &Path, scope: &str, path: &str) -> Option<ArchiveRecord> {
    archives(state_dir)
        .into_iter()
        .filter(|r| r.scope == scope && same_path(&r.path, path) && r.restored_at.is_none())
        .next_back()
}

/// Every regular file under `dir`, relative, sorted; a symlink is refused,
/// since what it points at is not the skill's to archive.
fn files_under(dir: &Path) -> std::result::Result<Vec<PathBuf>, String> {
    fn walk(base: &Path, at: &Path, out: &mut Vec<PathBuf>) -> std::result::Result<(), String> {
        let entries = std::fs::read_dir(at).map_err(|e| format!("reading {}: {e}", at.display()))?;
        for entry in entries.flatten() {
            let kind = entry.file_type().map_err(|e| format!("reading {}: {e}", entry.path().display()))?;
            if kind.is_symlink() {
                return Err(format!("{} is a symlink; a skill with one is not archived", entry.path().display()));
            }
            if kind.is_dir() {
                walk(base, &entry.path(), out)?;
            } else if kind.is_file() {
                out.push(entry.path().strip_prefix(base).unwrap_or(&entry.path()).to_path_buf());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// The files of a bundle, by relative path. Any entry whose path is not
/// plainly relative is refused, so nothing restores outside its directory.
fn unbundle(bytes: &[u8]) -> std::result::Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    let decoder = zstd::Decoder::new(bytes).map_err(|e| format!("zstd: {e}"))?;
    let mut tar = tar::Archive::new(decoder);
    let mut out = BTreeMap::new();
    for entry in tar.entries().map_err(|e| format!("reading the bundle: {e}"))? {
        let mut entry = entry.map_err(|e| format!("reading the bundle: {e}"))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().map_err(|e| format!("reading the bundle: {e}"))?.to_path_buf();
        if !path.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err(format!("the bundle holds {}, which is not inside the skill", path.display()));
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|e| format!("reading the bundle: {e}"))?;
        out.insert(path, bytes);
    }
    Ok(out)
}

/// The bundle's files are exactly the manifest's, byte for byte.
fn verify(record: &ArchiveRecord, files: &BTreeMap<PathBuf, Vec<u8>>) -> std::result::Result<(), String> {
    let listed: BTreeMap<PathBuf, &ArchivedFile> = record.files.iter().map(|f| (PathBuf::from(&f.path), f)).collect();
    if listed.len() != files.len() || listed.keys().ne(files.keys()) {
        return Err(format!("archive {} does not hold the files its manifest lists", record.id));
    }
    for (path, bytes) in files {
        if listed[path].sha256 != sha256(bytes) {
            return Err(format!("archive {}: {} does not match its manifest", record.id, path.display()));
        }
    }
    Ok(())
}

/// Archive a skill: bundle its directory, write the manifest, read both
/// back and check them against each other, and only then remove the
/// directory. A failure anywhere before that last step leaves the skill
/// where it was.
#[allow(clippy::too_many_arguments)]
pub fn archive(
    state_dir: &Path,
    scope: &str,
    declared: &str,
    name: &str,
    dir: &Path,
    last_used: Option<DateTime<Utc>>,
    reason: &str,
    now: DateTime<Utc>,
) -> std::result::Result<ArchiveRecord, String> {
    let paths = files_under(dir)?;
    let mut files = Vec::new();
    let encoder = zstd::Encoder::new(Vec::new(), 3).map_err(|e| format!("zstd: {e}"))?;
    let mut tar = tar::Builder::new(encoder);
    for rel in &paths {
        let bytes = std::fs::read(dir.join(rel)).map_err(|e| format!("reading {}: {e}", dir.join(rel).display()))?;
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(now.timestamp().max(0) as u64);
        header.set_cksum();
        tar.append_data(&mut header, rel, bytes.as_slice()).map_err(|e| format!("bundling {}: {e}", rel.display()))?;
        files.push(ArchivedFile { path: rel.display().to_string(), bytes: bytes.len() as u64, sha256: sha256(&bytes) });
    }
    let bundle = tar
        .into_inner()
        .and_then(|encoder| encoder.finish())
        .map_err(|e| format!("bundling {}: {e}", dir.display()))?;

    let base = archive_root(state_dir).join(component(scope));
    let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let mut leaf = format!("{name}-{stamp}");
    let mut n = 1;
    while base.join(&leaf).exists() {
        n += 1;
        leaf = format!("{name}-{stamp}-{n}");
    }
    let record = ArchiveRecord {
        id: format!("{}/{leaf}", component(scope)),
        name: name.into(),
        scope: scope.into(),
        path: declared.into(),
        dir: dir.display().to_string(),
        archived_at: now,
        last_used,
        reason: reason.into(),
        files,
        restored_at: None,
    };
    let partial = base.join(format!(".{leaf}.partial"));
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|e| format!("making {}: {e}", partial.display()))?;
    let manifest = serde_json::to_vec_pretty(&record).map_err(|e| format!("encoding the manifest: {e}"))?;
    std::fs::write(partial.join(BUNDLE), &bundle).map_err(|e| format!("writing the bundle: {e}"))?;
    std::fs::write(partial.join(MANIFEST), &manifest).map_err(|e| format!("writing the manifest: {e}"))?;
    let check = std::fs::read(partial.join(BUNDLE)).map_err(|e| format!("reading the bundle back: {e}"))?;
    if let Err(e) = unbundle(&check).and_then(|files| verify(&record, &files)) {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(format!("the archive of {name} did not verify, so the skill was left where it is: {e}"));
    }
    std::fs::rename(&partial, base.join(&leaf)).map_err(|e| format!("keeping the archive: {e}"))?;
    std::fs::remove_dir_all(dir).map_err(|e| format!("removing {} after archiving it: {e}", dir.display()))?;
    Ok(record)
}

/// Put an archived skill back where it was, from its bundle, checked
/// against its manifest first. Refused when something is there already.
pub fn restore(state_dir: &Path, id: &str, now: DateTime<Utc>) -> std::result::Result<ArchiveRecord, String> {
    let at = archive_root(state_dir).join(id);
    if !id.split('/').all(|part| !part.is_empty() && part != "." && part != "..") {
        return Err(format!("{id:?} is not an archive"));
    }
    let mut record: ArchiveRecord = std::fs::read(at.join(MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(|| format!("there is no archive {id:?}"))?;
    if let Some(when) = record.restored_at {
        return Err(format!("archive {id} was already restored, at {when}"));
    }
    let dir = PathBuf::from(&record.dir);
    if dir.exists() && std::fs::read_dir(&dir).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(format!("{} is there again already; restoring would overwrite it, so nothing was changed", dir.display()));
    }
    let bytes = std::fs::read(at.join(BUNDLE)).map_err(|e| format!("reading the bundle: {e}"))?;
    let files = unbundle(&bytes)?;
    verify(&record, &files)?;
    let parent = dir.parent().ok_or_else(|| format!("{} has no parent", dir.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("making {}: {e}", parent.display()))?;
    let staging = parent.join(format!(
        ".{}.factory-restore-{}",
        dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    for (rel, bytes) in &files {
        let path = staging.join(rel);
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("making {}: {e}", p.display()))?;
        }
        std::fs::write(&path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))?;
    }
    if files.is_empty() {
        std::fs::create_dir_all(&staging).map_err(|e| format!("making {}: {e}", staging.display()))?;
    }
    if dir.exists() {
        let _ = std::fs::remove_dir(&dir);
    }
    std::fs::rename(&staging, &dir).map_err(|e| {
        let _ = std::fs::remove_dir_all(&staging);
        format!("putting {} back: {e}", dir.display())
    })?;
    record.restored_at = Some(now);
    let manifest = serde_json::to_vec_pretty(&record).map_err(|e| format!("encoding the manifest: {e}"))?;
    write_atomically(&at.join(MANIFEST), &manifest)?;
    Ok(record)
}

/// The archive `restore <name>` means: the newest unrestored one of that
/// name, in `scope` when one is given. Two scopes with the same name are
/// refused, naming both, rather than guessed between.
pub fn archive_named(state_dir: &Path, name: &str, scope: Option<&str>) -> std::result::Result<ArchiveRecord, String> {
    let candidates: Vec<ArchiveRecord> = archives(state_dir)
        .into_iter()
        .filter(|r| r.name == name && r.restored_at.is_none() && scope.is_none_or(|s| r.scope == s))
        .collect();
    let scopes: BTreeSet<&str> = candidates.iter().map(|r| r.scope.as_str()).collect();
    match scopes.len() {
        0 => Err(format!("no archived skill is called {name:?}{}", scope.map(|s| format!(" in scope {s:?}")).unwrap_or_default())),
        1 => Ok(candidates.last().cloned().expect("one scope has an archive")),
        _ => Err(format!(
            "skills called {name:?} are archived in {}; say which with --scope",
            scopes.into_iter().collect::<Vec<_>>().join(" and ")
        )),
    }
}

// --------------------------------------------------------------- curation

/// The root config's `daemon.skill_curation`: when to curate, and how long
/// a skill may go unused before it is archived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurationConfig {
    pub schedule: CurationSchedule,
    #[serde(default = "default_unused_weeks")]
    pub unused_weeks: u32,
}

fn default_unused_weeks() -> u32 {
    12
}

/// A cron schedule, UTC unless it names a timezone -- the shape backup's is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CurationSchedule {
    pub cron: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
}

impl CurationSchedule {
    pub fn as_schedule(&self) -> factory_kernel::Schedule {
        factory_kernel::Schedule::Cron(factory_kernel::CronSchedule {
            expr: self.cron.clone(),
            timezone: self.timezone.clone(),
        })
    }
}

impl CurationConfig {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.unused_weeks == 0 {
            return Err("daemon.skill_curation.unused_weeks is 0, which would archive every skill on every pass".into());
        }
        factory_kernel::schedule_grid::next_after(&self.schedule.as_schedule(), Utc::now())
            .map(|_| ())
            .map_err(|e| format!("daemon.skill_curation.schedule: {e}"))
    }
}

/// When curation last ran, so a schedule catches up once after downtime.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurationStamp {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<DateTime<Utc>>,
}

impl CurationStamp {
    fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("curation.json")
    }
    pub fn load(state_dir: &Path) -> Self {
        std::fs::read(Self::path(state_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    pub fn save(&self, state_dir: &Path) -> std::result::Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| format!("encoding the curation stamp: {e}"))?;
        write_atomically(&Self::path(state_dir), &json)
    }
}

/// A skill curation found unused, and what it did about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unused {
    pub scope: String,
    pub name: String,
    pub path: String,
    /// Last handed to a run, or first seen when it never was.
    pub since: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What one curation pass found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CurationReport {
    pub at: DateTime<Utc>,
    pub unused_weeks: u32,
    pub dry_run: bool,
    /// Every skill unused for at least `unused_weeks`: archived, or (in a
    /// dry run) what would have been.
    pub unused: Vec<Unused>,
    /// Skills curation left alone: in use, or with a proposal waiting.
    pub kept: u32,
}

/// One pass: every declared skill whose directory is there and that no run
/// has been handed for `unused_weeks` is archived -- unless a proposal for
/// it is waiting on a person, which would only go stale. A dry run decides
/// the same and changes nothing.
pub fn curate(
    root: &Path,
    state_dir: &Path,
    scopes: &[MemoryScope],
    open: &BTreeSet<(String, String)>,
    unused_weeks: u32,
    now: DateTime<Utc>,
    dry_run: bool,
) -> CurationReport {
    let mut usage = UsageLog::load(state_dir);
    let cutoff = now - chrono::Duration::weeks(unused_weeks as i64);
    let mut report = CurationReport { at: now, unused_weeks, dry_run, unused: Vec::new(), kept: 0 };
    for scope in scopes {
        for decl in &scope.skills {
            let dir = scope.dir(root).join(decl.path.trim_end_matches('/'));
            if !dir.is_dir() {
                continue;
            }
            let name = name_of(decl, &dir);
            let seen = usage.seen(&scope.name, &decl.path, &name, now);
            let since = seen.since();
            if since > cutoff || open.contains(&(scope.name.clone(), name.clone())) {
                report.kept += 1;
                continue;
            }
            let mut unused = Unused { scope: scope.name.clone(), name: name.clone(), path: decl.path.clone(), since, archive: None, error: None };
            if !dry_run {
                let reason = format!("not handed to a run since {} ({} weeks or more)", since.format("%Y-%m-%d"), unused_weeks);
                match archive(state_dir, &scope.name, &decl.path, &name, &dir, seen.last_used, &reason, now) {
                    Ok(record) => unused.archive = Some(record.id),
                    Err(e) => unused.error = Some(e),
                }
            }
            report.unused.push(unused);
        }
    }
    if !dry_run {
        if let Err(e) = usage.save(state_dir) {
            tracing::warn!("saving skill usage: {e}");
        }
        if let Err(e) = (CurationStamp { last_run_at: Some(now) }).save(state_dir) {
            tracing::warn!("saving the curation stamp: {e}");
        }
    }
    report
}

/// One skill as `factory skill list` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRow {
    pub scope: String,
    pub name: String,
    pub path: String,
    pub dir: String,
    pub learn: Learn,
    pub present: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_seen: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run: Option<String>,
    pub open_proposals: u32,
}

/// Every declared skill, with what Factory knows of it.
pub fn rows(root: &Path, state_dir: &Path, scopes: &[MemoryScope], proposals: &[Proposal]) -> Vec<SkillRow> {
    let usage = UsageLog::load(state_dir);
    let mut out = Vec::new();
    for scope in scopes {
        for decl in &scope.skills {
            let dir = scope.dir(root).join(decl.path.trim_end_matches('/'));
            let present = dir.is_dir();
            let archived = latest_archive(state_dir, &scope.name, &decl.path);
            let name = skill_name(state_dir, &scope.name, decl, &dir);
            let used = usage.find(&scope.name, &decl.path);
            out.push(SkillRow {
                open_proposals: proposals
                    .iter()
                    .filter(|p| p.scope == scope.name && p.skill == name && p.state == ProposalState::Open)
                    .count() as u32,
                scope: scope.name.clone(),
                path: decl.path.clone(),
                dir: dir.display().to_string(),
                learn: decl.learn,
                present,
                archived: archived.filter(|_| !present).map(|r| r.id),
                first_seen: used.map(|u| u.first_seen),
                last_used: used.and_then(|u| u.last_used),
                last_run: used.and_then(|u| u.last_run.clone()),
                name,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("factory-skills-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("projects/radar/skill/references")).unwrap();
        std::fs::write(
            dir.join("projects/radar/skill/SKILL.md"),
            "---\nname: conference-radar-tracking\ndescription: Track CFPs.\n---\n\n# Steps\n1. Read the tracker.\n2. Search.\n3. Write.\n",
        )
        .unwrap();
        std::fs::write(dir.join("projects/radar/skill/references/sources.md"), "- sessionize\n").unwrap();
        dir
    }

    fn radar() -> MemoryScope {
        MemoryScope {
            name: "radar".into(),
            path: "projects/radar".into(),
            memory: None,
            skills: vec![SkillDecl { path: "skill/".into(), learn: Learn::Propose, name: None }],
        }
    }

    fn origin() -> Origin {
        Origin { task_id: "t1".into(), run_id: "r1".into(), agent: "scout".into() }
    }

    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn a_skill_is_named_by_its_declaration_its_frontmatter_or_its_directory() {
        let root = scratch("names");
        let scope = radar();
        let (decl, dir) = find(&root, &root.join(".factory/skills"), &scope, "conference-radar-tracking").unwrap();
        assert_eq!(name_of(decl, &dir), "conference-radar-tracking");
        let named = SkillDecl { name: Some("radar".into()), ..decl.clone() };
        assert_eq!(name_of(&named, &dir), "radar");
        let bare = SkillDecl { path: "other/".into(), learn: Learn::Off, name: None };
        assert_eq!(name_of(&bare, &root.join("projects/radar/other")), "other");
        assert_eq!(frontmatter_name("---\ndescription: x\nname: 'quoted-name'\n---\n"), Some("quoted-name".into()));
        assert_eq!(frontmatter_name("# no frontmatter\nname: x\n"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_diff_reads_like_diff_u() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\n";
        let new = "a\nb\nc\nD\ne\nf\ng\nh\ni\n";
        assert_eq!(
            unified_diff("SKILL.md", Some(old), Some(new)),
            "--- a/SKILL.md\n+++ b/SKILL.md\n@@ -1,8 +1,9 @@\n a\n b\n c\n-d\n+D\n e\n f\n g\n h\n+i\n"
        );
        assert_eq!(unified_diff("new.md", None, Some("x\n")), "--- /dev/null\n+++ b/new.md\n@@ -0,0 +1 @@\n+x\n");
        assert_eq!(unified_diff("gone.md", Some("x\ny\n"), None), "--- a/gone.md\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-x\n-y\n");
        let far = (1..=20).map(|i| format!("{i}\n")).collect::<String>();
        let edited = far.replace("\n2\n", "\ntwo\n").replace("\n19\n", "\nnineteen\n");
        let diff = unified_diff("f", Some(&far), Some(&edited));
        assert_eq!(diff.matches("@@ ").count(), 2, "two far-apart edits are two hunks:\n{diff}");
        assert!(unified_diff("f", Some("x"), Some("y")).contains("-x\n\\ No newline at end of file\n+y\n\\ No newline at end of file\n"));
    }

    #[test]
    fn a_proposal_is_a_diff_against_the_skill_as_it_was_and_applies_only_to_that() {
        let root = scratch("propose");
        let scope = radar();
        let (decl, dir) = find(&root, &root.join(".factory/skills"), &scope, "conference-radar-tracking").unwrap();
        let skill = std::fs::read_to_string(dir.join("SKILL.md")).unwrap();
        let submission = SkillSubmission {
            skill: "conference-radar-tracking".into(),
            summary: "Check sessionize's JSON API before scraping\nmore detail".into(),
            files: vec![
                SubmittedFile { path: "SKILL.md".into(), content: skill.replace("2. Search.", "2. Search; sessionize has a JSON API.") },
                SubmittedFile { path: "references/sources.md".into(), content: "- sessionize\n".into() },
                SubmittedFile { path: "templates/map.html".into(), content: "<svg/>\n".into() },
            ],
            remove: vec!["references/sources.md".into()],
            token: None,
        };
        let error = prepare("radar", decl, "conference-radar-tracking", &dir, &submission, origin(), "p1".into(), Utc::now(), &[]).unwrap_err();
        assert!(error.contains("both changed and removed"), "{error}");

        let submission = SkillSubmission { files: submission.files[..1].to_vec(), remove: Vec::new(), ..submission.clone() };
        let mut more = submission.clone();
        more.files.push(SubmittedFile { path: "templates/map.html".into(), content: "<svg/>\n".into() });
        let proposal = prepare("radar", decl, "conference-radar-tracking", &dir, &more, origin(), "p1".into(), Utc::now(), &[]).unwrap();
        assert_eq!(proposal.summary, "Check sessionize's JSON API before scraping");
        assert_eq!(proposal.changes.iter().map(|c| (c.path.as_str(), c.kind)).collect::<Vec<_>>(),
            vec![("SKILL.md", ChangeKind::Changed), ("templates/map.html", ChangeKind::Added)]);
        assert!(proposal.diff.contains("-2. Search.\n+2. Search; sessionize has a JSON API.\n"), "{}", proposal.diff);
        assert!(proposal.diff.contains("--- /dev/null\n+++ b/templates/map.html\n"));
        assert!(proposal.listing().changes.iter().all(|c| c.content.is_none()));

        let store = ProposalStore::new(&root.join(".factory/skills"));
        store.put(&proposal).unwrap();
        assert_eq!(store.get("p1").unwrap().unwrap(), proposal);
        assert_eq!(store.get("../../etc/passwd").unwrap(), None);

        apply_changes(&proposal).unwrap();
        assert!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap().contains("JSON API"));
        assert_eq!(std::fs::read_to_string(dir.join("templates/map.html")).unwrap(), "<svg/>\n");
        let error = apply_changes(&proposal).unwrap_err();
        assert!(error.contains("SKILL.md changed since this proposal was filed"), "{error}");

        let same = prepare("radar", decl, "conference-radar-tracking", &dir, &more, origin(), "p2".into(), Utc::now(), &[]).unwrap_err();
        assert!(same.contains("nothing to change"), "{same}");
        let escape = SkillSubmission { files: vec![SubmittedFile { path: "../AGENTS.md".into(), content: "x".into() }], ..submission.clone() };
        assert!(prepare("radar", decl, "x", &dir, &escape, origin(), "p3".into(), Utc::now(), &[]).unwrap_err().contains("no `..`"));
        let secret = SkillSubmission {
            files: vec![SubmittedFile { path: "notes.md".into(), content: "use ghp_0123456789abcdefghijABCDEFGHIJklmnop\n".into() }],
            ..submission
        };
        let error = prepare("radar", decl, "x", &dir, &secret, origin(), "p4".into(), Utc::now(), &[]).unwrap_err();
        assert!(error.contains("looks like it holds a secret -- a GitHub token"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn curation_archives_an_unused_skill_with_a_verified_backup_and_restore_brings_it_back() {
        let root = scratch("curate");
        let state = root.join(".factory/skills");
        let scopes = vec![radar()];
        let dir = root.join("projects/radar/skill");
        let before = files_under(&dir).unwrap();

        // First pass: never seen before, so unused only from now.
        let first = curate(&root, &state, &scopes, &BTreeSet::new(), 4, at("2026-01-01T00:00:00Z"), false);
        assert_eq!((first.unused.len(), first.kept), (0, 1));
        // A run used it a week later; five weeks after that, not yet unused.
        let mut usage = UsageLog::load(&state);
        usage.used("radar", "skill/", "conference-radar-tracking", "r1", at("2026-01-08T00:00:00Z"));
        usage.save(&state).unwrap();
        let soon = curate(&root, &state, &scopes, &BTreeSet::new(), 6, at("2026-02-12T00:00:00Z"), false);
        assert!(soon.unused.is_empty());
        // A proposal waiting on a person keeps it too.
        let open = BTreeSet::from([("radar".to_string(), "conference-radar-tracking".to_string())]);
        assert!(curate(&root, &state, &scopes, &open, 1, at("2026-06-01T00:00:00Z"), false).unused.is_empty());
        // A dry run says what it would do and does nothing.
        let dry = curate(&root, &state, &scopes, &BTreeSet::new(), 6, at("2026-06-01T00:00:00Z"), true);
        assert_eq!(dry.unused.len(), 1);
        assert!(dry.unused[0].archive.is_none() && dir.is_dir());

        let pass = curate(&root, &state, &scopes, &BTreeSet::new(), 6, at("2026-06-01T00:00:00Z"), false);
        let id = pass.unused[0].archive.clone().unwrap_or_else(|| panic!("{:?}", pass.unused[0].error));
        assert!(!dir.exists(), "the archived directory is gone from the scope");
        assert!(state.join("archive").join(&id).join("manifest.json").is_file());
        assert!(state.join("archive").join(&id).join("skill.tar.zst").is_file());
        let memory = crate::memory::read(&root, &state, &scopes[0]);
        assert_eq!(memory.skills[0].archived.as_deref(), Some(id.as_str()));
        let row = &rows(&root, &state, &scopes, &[])[0];
        assert_eq!((row.name.as_str(), row.present, row.archived.as_deref()), ("conference-radar-tracking", false, Some(id.as_str())));

        let record = archive_named(&state, "conference-radar-tracking", None).unwrap();
        assert_eq!(record.last_used, Some(at("2026-01-08T00:00:00Z")));
        restore(&state, &record.id, at("2026-06-02T00:00:00Z")).unwrap();
        assert_eq!(files_under(&dir).unwrap(), before);
        assert!(std::fs::read_to_string(dir.join("SKILL.md")).unwrap().contains("name: conference-radar-tracking"));
        assert!(restore(&state, &record.id, Utc::now()).unwrap_err().contains("already restored"));
        assert!(archive_named(&state, "conference-radar-tracking", None).unwrap_err().contains("no archived skill"));
        assert!(crate::memory::read(&root, &state, &scopes[0]).skills[0].present());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_tampered_archive_is_never_restored() {
        let root = scratch("tamper");
        let state = root.join(".factory/skills");
        let dir = root.join("projects/radar/skill");
        let record = archive(&state, "radar", "skill/", "conference-radar-tracking", &dir, None, "test", Utc::now()).unwrap();
        let manifest = archive_root(&state).join(&record.id).join("manifest.json");
        let mut edited: ArchiveRecord = serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
        edited.files[0].sha256 = "0".repeat(64);
        std::fs::write(&manifest, serde_json::to_vec(&edited).unwrap()).unwrap();
        assert!(restore(&state, &record.id, Utc::now()).unwrap_err().contains("does not match its manifest"));
        assert!(!dir.exists());
        assert!(restore(&state, "../../etc", Utc::now()).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
