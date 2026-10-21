//! A scope's memory (`#339`): two small files every run of the scope is
//! handed -- `facts`, what earlier runs recorded, and `profile`, who the
//! owner is -- each with a character budget a write may never exceed.
//!
//! ```yaml
//! scope:
//!   memory:
//!     facts:   { path: memory/MEMORY.md, max_chars: 2200 }
//!     profile: { path: memory/USER.md,   max_chars: 1400, shared: owner }
//! ```
//!
//! **Files, not rows.** The text lives where the config says, in the scope's
//! own directory (a `shared: owner` profile at the instance root instead), so
//! it is versioned with the project and readable by every harness. Factory
//! keeps no copy: the `ScopeMemory` fact is read fresh from the files.
//!
//! **Entries.** A file is entries separated by a line holding only `§` --
//! the format Hermes wrote, so its `MEMORY.md`/`USER.md` come across as they
//! are. A write adds, replaces or removes one whole entry; nothing else in
//! the file is ever rewritten, and nothing is ever truncated to fit. A write
//! that would go over budget is refused with the entries and their sizes,
//! oldest first, and which of them dropping would make room.
use factory_environment::secrets::SecretDecl;
pub use factory_kernel::{Learn, MemoryFile, MemoryTarget, ScopeMemory, SkillHandle};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// The line that separates two entries.
pub const SEPARATOR: &str = "§";

/// No memory file may be declared larger than this: it goes into every
/// prompt of the scope, and the point is a *small* memory.
pub const MAX_BUDGET: u32 = 20_000;

/// `scope.memory`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryDecl {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<MemoryFileDecl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<MemoryFileDecl>,
}

/// One of `scope.memory`'s two files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryFileDecl {
    /// Relative to the scope's directory -- or to the instance root, for a
    /// profile that is `shared: owner`.
    pub path: String,
    pub max_chars: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared: Option<Shared>,
}

/// Who a profile is shared with. One value today: the owner, whose single
/// profile at the instance root every scope saying so reads and writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shared {
    Owner,
}

/// One `scope.skills` entry: a directory holding `SKILL.md` and whatever it
/// refers to (`references/`, `templates/`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillDecl {
    /// Relative to the scope's directory.
    pub path: String,
    /// What a run may do with it. `propose` when left out.
    #[serde(default)]
    pub learn: Learn,
    /// What it is called. Left out, the `name:` in `SKILL.md`'s frontmatter,
    /// else the directory's own name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// What the memory and skills code needs to know about one scope: plain
/// declarations, captured fresh from the configuration for each read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryScope {
    pub name: String,
    /// Relative to the instance root, as `Scope.path` is (`.` for the root).
    pub path: PathBuf,
    pub memory: Option<MemoryDecl>,
    pub skills: Vec<SkillDecl>,
}

impl MemoryScope {
    pub fn dir(&self, root: &Path) -> PathBuf {
        if self.path.as_os_str().is_empty() || self.path == Path::new(".") {
            root.to_path_buf()
        } else {
            root.join(&self.path)
        }
    }

    /// The declaration for `target`, and where its file is.
    pub fn file(&self, root: &Path, target: MemoryTarget) -> Option<(&MemoryFileDecl, PathBuf)> {
        let memory = self.memory.as_ref()?;
        let decl = match target {
            MemoryTarget::Facts => memory.facts.as_ref()?,
            MemoryTarget::Profile => memory.profile.as_ref()?,
        };
        let base = if decl.shared.is_some() { root.to_path_buf() } else { self.dir(root) };
        Some((decl, base.join(decl.path.trim_end_matches('/'))))
    }

    pub fn is_empty(&self) -> bool {
        self.memory.as_ref().is_none_or(|m| m.facts.is_none() && m.profile.is_none()) && self.skills.is_empty()
    }
}

// ------------------------------------------------------------ validation

/// A path a scope may name for its memory or a skill: relative, inside the
/// directory it is relative to, and never Factory's own `.factory/` or git's
/// `.git/`.
pub fn relative_inside(path: &str) -> std::result::Result<PathBuf, String> {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("is empty".into());
    }
    let candidate = Path::new(trimmed);
    let mut out = PathBuf::new();
    for (i, component) in candidate.components().enumerate() {
        match component {
            Component::Normal(part) => {
                if i == 0 && (part == ".factory" || part == ".git") {
                    return Err(format!("is inside {}, which is not the scope's to write", part.to_string_lossy()));
                }
                out.push(part);
            }
            Component::CurDir => {}
            _ => return Err("must be relative and stay inside its directory (no `/` at the start, no `..`)".into()),
        }
    }
    if out.as_os_str().is_empty() {
        return Err("names the directory itself, not something in it".into());
    }
    Ok(out)
}

/// A skill's name: what `factory skill restore <name>` and a proposal name.
pub fn is_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        && !name.starts_with('.')
}

/// Check one scope's `memory:` and `skills:`. `where_` names it in errors.
pub fn validate(where_: &str, memory: Option<&MemoryDecl>, skills: &[SkillDecl]) -> std::result::Result<(), String> {
    if let Some(memory) = memory {
        for (label, decl) in [("facts", &memory.facts), ("profile", &memory.profile)] {
            let Some(decl) = decl else { continue };
            relative_inside(&decl.path).map_err(|e| format!("{where_}: memory.{label}.path {:?} {e}", decl.path))?;
            if decl.max_chars == 0 || decl.max_chars > MAX_BUDGET {
                return Err(format!(
                    "{where_}: memory.{label}.max_chars is {}, and has to be between 1 and {MAX_BUDGET}",
                    decl.max_chars
                ));
            }
            if label == "facts" && decl.shared.is_some() {
                return Err(format!(
                    "{where_}: memory.facts cannot be shared -- facts are the scope's own; only memory.profile can be `shared: owner`"
                ));
            }
        }
    }
    let mut names = std::collections::BTreeSet::new();
    let mut paths = std::collections::BTreeSet::new();
    for skill in skills {
        let path = relative_inside(&skill.path).map_err(|e| format!("{where_}: skills path {:?} {e}", skill.path))?;
        if !paths.insert(path.clone()) {
            return Err(format!("{where_}: skills names {} twice", path.display()));
        }
        if let Some(name) = &skill.name {
            if !is_skill_name(name) {
                return Err(format!(
                    "{where_}: skill name {name:?} has to be letters, digits, `-`, `_` or `.`, at most 64 characters"
                ));
            }
            if !names.insert(name.clone()) {
                return Err(format!("{where_}: two skills are both named {name:?}"));
            }
        }
    }
    Ok(())
}

/// Every scope that says `profile: { shared: owner }` reads the same file,
/// so they have to agree on which one and how large it may grow. Refuses,
/// naming both scopes, when two do not.
pub fn validate_owner_profile<'a>(
    scopes: impl IntoIterator<Item = (&'a str, &'a MemoryDecl)>,
) -> std::result::Result<(), String> {
    let mut first: Option<(&str, &MemoryFileDecl)> = None;
    for (scope, memory) in scopes {
        let Some(profile) = memory.profile.as_ref().filter(|p| p.shared == Some(Shared::Owner)) else { continue };
        match first {
            None => first = Some((scope, profile)),
            Some((other, seen)) => {
                let same_path = relative_inside(&seen.path).ok() == relative_inside(&profile.path).ok();
                if !same_path || seen.max_chars != profile.max_chars {
                    return Err(format!(
                        "scopes {other:?} and {scope:?} both share the owner profile but disagree on it \
                         ({} with max_chars {} against {} with max_chars {}); there is one owner profile, \
                         so every `shared: owner` has to name the same path and budget",
                        seen.path, seen.max_chars, profile.path, profile.max_chars
                    ));
                }
            }
        }
    }
    Ok(())
}

// --------------------------------------------------------------- entries

/// The entries of a memory file, in file order. Blank entries are dropped.
pub fn entries(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if line.trim() == SEPARATOR {
            push_entry(&mut out, &current);
            current.clear();
        } else {
            current.push_str(line);
            current.push('\n');
        }
    }
    push_entry(&mut out, &current);
    out
}

fn push_entry(out: &mut Vec<String>, text: &str) {
    let text = text.trim();
    if !text.is_empty() {
        out.push(text.to_string());
    }
}

/// The entries as they are written to the file, without its final newline.
pub fn body(entries: &[String]) -> String {
    entries.join(&format!("\n{SEPARATOR}\n"))
}

/// How many characters the entries take, the way the budget counts them:
/// characters, not bytes, separators included.
pub fn chars(entries: &[String]) -> usize {
    body(entries).chars().count()
}

/// The file's full text.
pub fn render(entries: &[String]) -> String {
    if entries.is_empty() {
        String::new()
    } else {
        format!("{}\n", body(entries))
    }
}

/// What a separator costs between two entries: `\n§\n`.
const SEPARATOR_CHARS: usize = 3;

// ----------------------------------------------------------------- writes

/// `add`, `replace` or `remove`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryOp {
    Add,
    Replace,
    Remove,
}

impl MemoryOp {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Replace => "replace",
            Self::Remove => "remove",
        }
    }
}

/// `factory memory add|replace|remove`, as it travels to the daemon. The
/// scope is never named: it is always the run's own task's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryWrite {
    #[serde(default)]
    pub target: MemoryTarget,
    pub op: MemoryOp,
    /// The entry's text, for `add` and `replace`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Which entry, by its 1-based position, for `replace` and `remove`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<u32>,
    /// Which entry, by text only it contains, for `replace` and `remove`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matching: Option<String>,
    /// The run token, exactly as `task report` carries it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

/// What a write did: the acknowledgement L3 returns and L4 journals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryReceipt {
    pub target: MemoryTarget,
    pub op: MemoryOp,
    pub path: String,
    /// The 1-based position of the entry written or removed.
    pub entry: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<String>,
    pub chars: u32,
    pub max_chars: u32,
    pub entries: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shared: bool,
}

/// What `apply` decided, before anything touches a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub entries: Vec<String>,
    /// 0-based.
    pub index: usize,
    pub old: Option<String>,
    pub new: Option<String>,
}

fn label(target: MemoryTarget) -> &'static str {
    match target {
        MemoryTarget::Facts => "This scope's facts",
        MemoryTarget::Profile => "The owner profile",
    }
}

/// One line of an entry, for a list a person or an agent reads.
pub fn preview(entry: &str) -> String {
    let flat: String = entry.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= 72 {
        flat
    } else {
        format!("{}…", flat.chars().take(71).collect::<String>())
    }
}

fn listing(entries: &[String]) -> String {
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| format!("  [{}] {} chars: {}\n", i + 1, e.chars().count(), preview(e)))
        .collect()
}

fn normalized(text: Option<&str>, max_chars: usize) -> std::result::Result<String, String> {
    let text = text.map(str::trim).unwrap_or_default();
    if text.is_empty() {
        return Err("an entry needs some text; nothing was written".into());
    }
    if text.lines().any(|line| line.trim() == SEPARATOR) {
        return Err(format!(
            "an entry cannot hold a line that is only {SEPARATOR}: that line is what separates entries; nothing was written"
        ));
    }
    let len = text.chars().count();
    if len > max_chars {
        return Err(format!(
            "this entry alone is {len} characters and the whole budget is {max_chars}; shorten it -- nothing was written, \
             and nothing is ever cut to fit"
        ));
    }
    Ok(text.to_string())
}

/// Which entry `write` names, 0-based.
fn selected(entries: &[String], write: &MemoryWrite, target: MemoryTarget) -> std::result::Result<usize, String> {
    if entries.is_empty() {
        return Err(format!("{} has no entries yet, so there is nothing to {}", label(target), write.op.as_str()));
    }
    if let Some(n) = write.entry {
        let n = n as usize;
        if n == 0 || n > entries.len() {
            return Err(format!(
                "there is no entry [{n}]: {} has {} entries --\n{}",
                label(target),
                entries.len(),
                listing(entries)
            ));
        }
        return Ok(n - 1);
    }
    let Some(needle) = write.matching.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
        return Err("say which entry: --entry <n> (its number in the list) or --match \"<text only it contains>\"".into());
    };
    let hits: Vec<usize> = entries.iter().enumerate().filter(|(_, e)| e.contains(needle)).map(|(i, _)| i).collect();
    match hits.as_slice() {
        [one] => Ok(*one),
        [] => Err(format!("no entry contains {needle:?}; the entries are:\n{}", listing(entries))),
        many => Err(format!(
            "{} entries contain {needle:?} -- give more of the text, or --entry <n>:\n{}",
            many.len(),
            many.iter().map(|i| format!("  [{}] {}\n", i + 1, preview(&entries[*i]))).collect::<String>()
        )),
    }
}

/// The answer to a write that does not fit: what it would come to, every
/// entry with its size, and the fewest oldest entries whose removal makes
/// room. `keep` is the entry being replaced, which cannot be the answer.
fn over_budget(target: MemoryTarget, entries: &[String], would_be: usize, max: usize, keep: Option<usize>) -> String {
    let over = would_be - max;
    let mut freed = 0;
    let mut drop = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if Some(i) == keep {
            continue;
        }
        if freed >= over {
            break;
        }
        freed += entry.chars().count() + SEPARATOR_CHARS;
        drop.push(i + 1);
    }
    let advice = if freed >= over {
        let names: Vec<String> = drop.iter().map(|n| format!("[{n}]")).collect();
        format!(
            "Removing {} (the oldest) would make room; or shorten whichever entries matter less.",
            names.join(" and ")
        )
    } else {
        "Even removing every other entry would not make room: shorten the new entry.".into()
    };
    format!(
        "{} would be {would_be} of {max} characters, {over} over budget, so nothing was written -- nothing is ever \
         cut to fit. Make room first with `factory memory remove` (or `replace` an entry with a shorter one), \
         then try again. The entries, oldest first:\n{}{advice}",
        label(target),
        listing(entries)
    )
}

/// Decide what `write` does to `entries` under `max_chars`, touching
/// nothing. Every refusal says why and what would work instead.
pub fn apply(
    entries: &[String],
    write: &MemoryWrite,
    target: MemoryTarget,
    max_chars: usize,
) -> std::result::Result<Change, String> {
    let mut next = entries.to_vec();
    match write.op {
        MemoryOp::Add => {
            let text = normalized(write.text.as_deref(), max_chars)?;
            if let Some(i) = entries.iter().position(|e| *e == text) {
                return Err(format!("that is already entry [{}]; nothing was written", i + 1));
            }
            next.push(text.clone());
            let would_be = chars(&next);
            if would_be > max_chars {
                return Err(over_budget(target, entries, would_be, max_chars, None));
            }
            Ok(Change { index: next.len() - 1, entries: next, old: None, new: Some(text) })
        }
        MemoryOp::Replace => {
            let index = selected(entries, write, target)?;
            let text = normalized(write.text.as_deref(), max_chars)?;
            if entries[index] == text {
                return Err(format!("entry [{}] already reads exactly that; nothing was written", index + 1));
            }
            let old = std::mem::replace(&mut next[index], text.clone());
            let would_be = chars(&next);
            if would_be > max_chars {
                return Err(over_budget(target, entries, would_be, max_chars, Some(index)));
            }
            Ok(Change { entries: next, index, old: Some(old), new: Some(text) })
        }
        MemoryOp::Remove => {
            if write.text.is_some() {
                return Err("remove takes no text -- name the entry with --entry or --match".into());
            }
            let index = selected(entries, write, target)?;
            let old = next.remove(index);
            Ok(Change { entries: next, index, old: Some(old), new: None })
        }
    }
}

/// Read, decide, check for secrets, write -- one memory write, whole. The
/// file is replaced by a rename, so a reader sees the old file or the new
/// one and never half of either. The caller serialises writes to a file.
pub fn write(
    root: &Path,
    scope: &MemoryScope,
    write: &MemoryWrite,
    catalogue: &[SecretDecl],
) -> std::result::Result<MemoryReceipt, String> {
    let target = write.target;
    let Some((decl, path)) = scope.file(root, target) else {
        return Err(format!(
            "scope {:?} declares no memory.{target}, so there is nowhere to write it -- \
             add `memory: {{ {target}: {{ path: ..., max_chars: ... }} }}` under its scope: block",
            scope.name
        ));
    };
    let text = read_text(&path)?;
    let current = entries(&text);
    let change = apply(&current, write, target, decl.max_chars as usize)?;
    if let Some(new) = &change.new {
        if let Some(finding) = factory_environment::secret_scan::find(new, catalogue) {
            return Err(format!(
                "this looks like a secret -- {finding} -- so nothing was written. Memory is handed to every later \
                 run and never holds a credential: say where it is kept (a secrets catalogue entry, a file) instead"
            ));
        }
    }
    write_atomically(&path, render(&change.entries).as_bytes())?;
    Ok(MemoryReceipt {
        target,
        op: write.op,
        path: path.display().to_string(),
        entry: change.index as u32 + 1,
        old: change.old,
        new: change.new,
        chars: chars(&change.entries) as u32,
        max_chars: decl.max_chars,
        entries: change.entries.len() as u32,
        shared: decl.shared.is_some(),
    })
}

/// A file's text, or `""` when it does not exist yet.
pub fn read_text(path: &Path) -> std::result::Result<String, String> {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8 text", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("reading {}: {e}", path.display())),
    }
}

/// Replace `path` with `bytes` by writing a sibling and renaming it over,
/// keeping the old file's permissions. A new file is the owner's alone
/// (0600): memory is often personal.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> std::result::Result<(), String> {
    let parent = path.parent().ok_or_else(|| format!("{} has no directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("making {}: {e}", parent.display()))?;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let temp = parent.join(format!(".{name}.factory-{}", std::process::id()));
    std::fs::write(&temp, bytes).map_err(|e| format!("writing {}: {e}", temp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).map(|m| m.permissions().mode() & 0o7777).unwrap_or(0o600);
        let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(mode));
    }
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!("replacing {}: {e}", path.display())
    })
}

// -------------------------------------------------------------- the fact

/// One memory file as the fact carries it.
fn memory_file(decl: &MemoryFileDecl, path: &Path) -> MemoryFile {
    let (entries, error) = match read_text(path) {
        Ok(text) => (entries(&text), None),
        Err(e) => (Vec::new(), Some(e)),
    };
    MemoryFile {
        path: path.display().to_string(),
        max_chars: decl.max_chars,
        chars: chars(&entries) as u32,
        entries,
        shared: decl.shared.is_some(),
        error,
    }
}

/// A scope's memory and skills, read from its files now.
pub fn read(root: &Path, state_dir: &Path, scope: &MemoryScope) -> ScopeMemory {
    let file = |target| scope.file(root, target).map(|(decl, path)| memory_file(decl, &path));
    let skills = scope
        .skills
        .iter()
        .map(|decl| {
            let dir = scope.dir(root).join(decl.path.trim_end_matches('/'));
            let name = crate::skills::skill_name(state_dir, &scope.name, decl, &dir);
            let (archived, error) = if dir.is_dir() {
                (None, None)
            } else {
                match crate::skills::latest_archive(state_dir, &scope.name, &decl.path) {
                    Some(record) => (Some(record.id), None),
                    None => (None, Some(format!("{} is not there", dir.display()))),
                }
            };
            SkillHandle { name, path: dir.display().to_string(), learn: decl.learn, archived, error }
        })
        .collect();
    ScopeMemory { facts: file(MemoryTarget::Facts), profile: file(MemoryTarget::Profile), skills }
}

/// L3's provider for the `ScopeMemory` fact. Constructed fresh per read from
/// the configuration's plain declarations; it reads the files, never a copy.
pub struct Provider {
    pub root: PathBuf,
    /// `<root>/.factory/skills`: where archives are, to say a missing skill
    /// directory is archived rather than lost.
    pub state_dir: PathBuf,
    pub scopes: Vec<MemoryScope>,
}

impl factory_kernel::FactProvider for Provider {
    type Level = factory_kernel::L3;
}

#[async_trait::async_trait]
impl factory_kernel::Provide<ScopeMemory> for Provider {
    /// The scope's canonical name.
    type Query = String;
    type Value = ScopeMemory;
    type Error = factory_kernel::FactoryError;
    async fn get(&self, scope: &String) -> factory_kernel::Result<ScopeMemory> {
        let Some(scope) = self.scopes.iter().find(|s| s.name == *scope).cloned() else {
            return Ok(ScopeMemory::default());
        };
        if scope.is_empty() {
            return Ok(ScopeMemory::default());
        }
        let (root, state_dir) = (self.root.clone(), self.state_dir.clone());
        tokio::task::spawn_blocking(move || read(&root, &state_dir, &scope))
            .await
            .map_err(|e| factory_kernel::FactoryError::Other(anyhow::anyhow!("reading scope memory: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add(text: &str) -> MemoryWrite {
        MemoryWrite { target: MemoryTarget::Facts, op: MemoryOp::Add, text: Some(text.into()), entry: None, matching: None, token: None }
    }

    fn owned(entries: &[&str]) -> Vec<String> {
        entries.iter().map(|e| e.to_string()).collect()
    }

    #[test]
    fn hermes_files_read_as_entries_and_write_back_byte_for_byte() {
        let text = "First fact, over\ntwo lines.\n§\nSecond fact.\n§\nThird: Ingo was accepted at DevLand 2027.\n";
        let parsed = entries(text);
        assert_eq!(parsed, owned(&["First fact, over\ntwo lines.", "Second fact.", "Third: Ingo was accepted at DevLand 2027."]));
        assert_eq!(render(&parsed), text);
        assert_eq!(chars(&parsed), text.chars().count() - 1);
        assert_eq!(entries(""), Vec::<String>::new());
        assert_eq!(entries("\n§\n\n§\nonly one\n"), owned(&["only one"]));
        // Characters, not bytes: `§` and umlauts count once.
        assert_eq!(chars(&owned(&["ä", "ö"])), 5);
    }

    #[test]
    fn add_replace_and_remove_change_exactly_one_entry() {
        let start = owned(&["alpha", "beta", "gamma"]);
        let added = apply(&start, &add("delta"), MemoryTarget::Facts, 100).unwrap();
        assert_eq!(added.entries, owned(&["alpha", "beta", "gamma", "delta"]));
        assert_eq!((added.index, added.old, added.new.as_deref()), (3, None, Some("delta")));

        let replace = MemoryWrite { op: MemoryOp::Replace, matching: Some("bet".into()), ..add("BETA") };
        let replaced = apply(&start, &replace, MemoryTarget::Facts, 100).unwrap();
        assert_eq!(replaced.entries, owned(&["alpha", "BETA", "gamma"]));
        assert_eq!(replaced.old.as_deref(), Some("beta"));

        let remove = MemoryWrite { op: MemoryOp::Remove, text: None, entry: Some(1), ..add("") };
        let removed = apply(&start, &remove, MemoryTarget::Facts, 100).unwrap();
        assert_eq!(removed.entries, owned(&["beta", "gamma"]));
        assert_eq!(removed.old.as_deref(), Some("alpha"));
    }

    #[test]
    fn a_write_over_budget_is_refused_with_what_to_drop_never_truncated() {
        let start = owned(&["a".repeat(30).as_str(), "b".repeat(30).as_str(), "c".repeat(30).as_str()]);
        assert_eq!(chars(&start), 96);
        let error = apply(&start, &add(&"d".repeat(30)), MemoryTarget::Facts, 110).unwrap_err();
        assert!(error.contains("would be 129 of 110 characters, 19 over budget, so nothing was written"), "{error}");
        assert!(error.contains("[1] 30 chars: aaaa"), "{error}");
        assert!(error.contains("[3] 30 chars: cccc"), "{error}");
        assert!(error.contains("Removing [1] (the oldest) would make room"), "{error}");

        // Replacing the oldest cannot be the answer to replacing it.
        let replace = MemoryWrite { op: MemoryOp::Replace, entry: Some(1), ..add(&"z".repeat(60)) };
        let error = apply(&start, &replace, MemoryTarget::Facts, 110).unwrap_err();
        assert!(error.contains("Removing [2] (the oldest) would make room"), "{error}");

        let error = apply(&start, &add(&"e".repeat(120)), MemoryTarget::Facts, 110).unwrap_err();
        assert!(error.contains("this entry alone is 120 characters"), "{error}");

        let error = apply(&owned(&["x".repeat(100).as_str()]), &add(&"y".repeat(100)), MemoryTarget::Facts, 150).unwrap_err();
        assert!(error.contains("Removing [1] (the oldest) would make room"), "{error}");
    }

    #[test]
    fn ambiguous_missing_duplicate_and_separator_writes_are_refused_by_name() {
        let start = owned(&["Ingo prefers German", "Ingo speaks at conferences"]);
        let replace = MemoryWrite { op: MemoryOp::Replace, matching: Some("Ingo".into()), ..add("x") };
        let error = apply(&start, &replace, MemoryTarget::Profile, 500).unwrap_err();
        assert!(error.starts_with("2 entries contain \"Ingo\""), "{error}");
        let error = apply(&start, &MemoryWrite { matching: Some("Berlin".into()), ..replace.clone() }, MemoryTarget::Profile, 500).unwrap_err();
        assert!(error.contains("no entry contains \"Berlin\""), "{error}");
        let error = apply(&start, &MemoryWrite { matching: None, ..replace.clone() }, MemoryTarget::Profile, 500).unwrap_err();
        assert!(error.contains("--entry <n>"), "{error}");
        let error = apply(&start, &MemoryWrite { matching: None, entry: Some(3), ..replace }, MemoryTarget::Profile, 500).unwrap_err();
        assert!(error.contains("there is no entry [3]"), "{error}");
        assert!(apply(&start, &add("Ingo prefers German"), MemoryTarget::Profile, 500).unwrap_err().contains("already entry [1]"));
        assert!(apply(&start, &add("one\n§\ntwo"), MemoryTarget::Profile, 500).unwrap_err().contains("separates entries"));
        assert!(apply(&start, &add("  "), MemoryTarget::Profile, 500).unwrap_err().contains("needs some text"));
        assert!(apply(&[], &MemoryWrite { op: MemoryOp::Remove, text: None, entry: Some(1), ..add("") }, MemoryTarget::Facts, 5)
            .unwrap_err().contains("has no entries yet"));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("factory-memory-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("projects/radar")).unwrap();
        dir
    }

    fn radar(memory: MemoryDecl) -> MemoryScope {
        MemoryScope { name: "radar".into(), path: "projects/radar".into(), memory: Some(memory), skills: Vec::new() }
    }

    fn decl(path: &str, max: u32, shared: bool) -> Option<MemoryFileDecl> {
        Some(MemoryFileDecl { path: path.into(), max_chars: max, shared: shared.then_some(Shared::Owner) })
    }

    #[test]
    fn a_write_lands_in_the_scope_or_the_shared_profile_at_the_root_and_never_holds_a_secret() {
        let root = scratch("write");
        let scope = radar(MemoryDecl { facts: decl("memory/MEMORY.md", 200, false), profile: decl("memory/USER.md", 100, true) });
        let receipt = write(&root, &scope, &add("Accepted at DevLand 2027."), &[]).unwrap();
        assert_eq!(receipt.entry, 1);
        assert_eq!((receipt.chars, receipt.max_chars, receipt.entries), (25, 200, 1));
        let facts = root.join("projects/radar/memory/MEMORY.md");
        assert_eq!(std::fs::read_to_string(&facts).unwrap(), "Accepted at DevLand 2027.\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&facts).unwrap().permissions().mode() & 0o777, 0o600);
        }

        let profile = MemoryWrite { target: MemoryTarget::Profile, ..add("Prefers to be asked in German.") };
        let receipt = write(&root, &scope, &profile, &[]).unwrap();
        assert!(receipt.shared);
        assert_eq!(std::fs::read_to_string(root.join("memory/USER.md")).unwrap(), "Prefers to be asked in German.\n");

        let error = write(&root, &scope, &add("the key is sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWx"), &[]).unwrap_err();
        assert!(error.contains("looks like a secret -- an Anthropic API key"), "{error}");
        assert!(!error.contains("AbCdEf"), "a refusal never repeats the secret: {error}");
        assert_eq!(std::fs::read_to_string(&facts).unwrap(), "Accepted at DevLand 2027.\n", "nothing written");

        let none = MemoryScope { memory: None, ..scope.clone() };
        assert!(write(&root, &none, &add("x"), &[]).unwrap_err().contains("declares no memory.facts"));

        let memory = read(&root, &root.join(".factory/skills"), &scope);
        assert_eq!(memory.facts.as_ref().unwrap().entries, owned(&["Accepted at DevLand 2027."]));
        assert_eq!(memory.profile.as_ref().unwrap().chars, 30);
        assert!(memory.profile.as_ref().unwrap().shared);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn declarations_are_checked_where_they_are_written() {
        let ok = MemoryDecl { facts: decl("memory/MEMORY.md", 2200, false), profile: decl("memory/USER.md", 1400, true) };
        assert!(validate("radar", Some(&ok), &[]).is_ok());
        for (memory, expected) in [
            (MemoryDecl { facts: decl("../MEMORY.md", 10, false), profile: None }, "no `..`"),
            (MemoryDecl { facts: decl("/etc/MEMORY.md", 10, false), profile: None }, "must be relative"),
            (MemoryDecl { facts: decl(".factory/MEMORY.md", 10, false), profile: None }, "inside .factory"),
            (MemoryDecl { facts: decl("MEMORY.md", 0, false), profile: None }, "between 1 and"),
            (MemoryDecl { facts: decl("MEMORY.md", 10, true), profile: None }, "cannot be shared"),
        ] {
            let error = validate("radar", Some(&memory), &[]).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }
        let skill = |path: &str, name: Option<&str>| SkillDecl { path: path.into(), learn: Learn::Propose, name: name.map(Into::into) };
        assert!(validate("radar", None, &[skill("skill/", None), skill("other", Some("b"))]).is_ok());
        assert!(validate("radar", None, &[skill("skill/", None), skill("skill", None)]).unwrap_err().contains("twice"));
        assert!(validate("radar", None, &[skill("a", Some("x y"))]).unwrap_err().contains("letters, digits"));

        let other = MemoryDecl { facts: None, profile: decl("memory/OWNER.md", 1400, true) };
        assert!(validate_owner_profile([("radar", &ok), ("again", &ok)]).is_ok());
        let error = validate_owner_profile([("radar", &ok), ("finance", &other)]).unwrap_err();
        assert!(error.contains("scopes \"radar\" and \"finance\" both share the owner profile"), "{error}");
    }
}
