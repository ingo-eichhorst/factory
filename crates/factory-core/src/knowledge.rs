//! A read-only index of the instance's wiki, rebuilt from the files on every
//! request. There is no link table to keep in step and nothing is ever
//! written: `index` walks `<root>/knowledge/wiki/` and returns titles,
//! frontmatter fields, links and findings. **No note body text ever appears
//! in the result** -- a reader who wants the text opens the file itself.
//!
//! `index` is a pure function of what is on disk: the same files always
//! produce the same bytes. It is meant to run inside `spawn_blocking` (see
//! `factory-daemon/src/engine.rs`) because a large wiki is a filesystem walk,
//! not something to do on the async runtime's own thread.
//!
//! See `knowledge/SCHEMA.md` in an instance root for the format this reads,
//! and the module's unit tests for the resolution rules a real wiki needs
//! that the schema does not spell out.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

/// No wiki this large has been tried. The cap exists so a request against one
/// that is stops in bounded time instead of walking forever; hitting it is a
/// `truncated` finding, never an error.
const MAX_FILES: usize = 2_000;

/// A file over this size is not read at all -- its frontmatter and links are
/// unknown, and it is filed as a page rather than guessed at. 1 MiB.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// One note: a `.md` file whose frontmatter parsed and named a `title`. Its
/// id is its path under the wiki root, `/`-separated, without `.md`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Note {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub area: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub updated: Option<String>,
    /// A count, never the list: the payload does not carry a source path
    /// unless a finding names it, and a finding never names one that lies
    /// under `data/secrets/`.
    pub sources: usize,
    /// Resolved note ids this note points at, deduplicated and sorted.
    pub links: Vec<String>,
    /// Targets this note points at that never resolved to a note -- as
    /// written, after stripping `|label`/`#heading` -- deduplicated and
    /// sorted.
    pub gaps: Vec<String>,
    /// Every note whose own `links` resolves here, sorted.
    pub backlinks: Vec<String>,
}

/// A target written somewhere in the wiki that no note answers: nothing was
/// ever written for it, the name did not resolve, or it resolved to a page
/// rather than a note.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Gap {
    pub target: String,
    pub from: Vec<String>,
}

/// One of the seven things the indexer checks for. Serializes to exactly the
/// strings the wire format names. Declared in the order the issue's own
/// table lists them, which is also the order findings sort in.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    Unsourced,
    SecretSource,
    MissingSource,
    IncompleteFrontmatter,
    Orphan,
    AmbiguousLink,
    Truncated,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Finding {
    pub kind: FindingKind,
    /// The note the finding is about. Empty for a `truncated` finding that
    /// belongs to the walk as a whole (the 2,000-file cap) rather than one
    /// file; a `truncated` finding for one oversized file names its path
    /// here instead, since the file need not be a note at all.
    pub note: String,
    pub detail: String,
}

/// The whole answer to `GET /api/knowledge`, minus the `"kind"` tag that
/// `Payload::Knowledge` adds on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Index {
    /// The absolute path this index was built from.
    pub root: String,
    /// `false` when `<root>/knowledge/wiki` does not exist. Not an error --
    /// an instance with no wiki yet is an empty state.
    pub present: bool,
    pub notes: Vec<Note>,
    /// Sorted by descending pointer count, then by target.
    pub gaps: Vec<Gap>,
    /// Every `.md` file that is not a note, by path, so nothing under the
    /// wiki root is silently dropped from the payload.
    pub pages: Vec<String>,
    pub findings: Vec<Finding>,
}

/// Walk `<root>/knowledge/wiki` and build the index. Never fails: a missing
/// directory is `present: false`, and a walk limit hit is a `truncated`
/// finding rather than an error.
pub fn index(root: &Path) -> Index {
    let wiki_root = root.join("knowledge").join("wiki");
    let root_display = wiki_root.display().to_string();
    if !wiki_root.is_dir() {
        return Index {
            root: root_display,
            present: false,
            notes: Vec::new(),
            gaps: Vec::new(),
            pages: Vec::new(),
            findings: Vec::new(),
        };
    }

    let (files, walk_truncated) = discover(&wiki_root);

    let mut notes: Vec<NoteDraft> = Vec::new();
    let mut pages: Vec<String> = Vec::new();
    let mut findings: Vec<Finding> = Vec::new();

    for (rel, abs) in &files {
        let id = rel[..rel.len() - 3].to_string();
        let Ok(meta) = fs::metadata(abs) else {
            pages.push(rel.clone());
            continue;
        };
        if meta.len() > MAX_FILE_BYTES {
            findings.push(Finding {
                kind: FindingKind::Truncated,
                note: rel.clone(),
                detail: "file exceeds the 1 MiB cap and was not read".into(),
            });
            pages.push(rel.clone());
            continue;
        }
        let Ok(content) = fs::read_to_string(abs) else {
            pages.push(rel.clone());
            continue;
        };
        let Some((yaml, body)) = split_frontmatter(&content) else {
            pages.push(rel.clone());
            continue;
        };
        let Ok(fm) = serde_yaml_ng::from_str::<RawFrontMatter>(yaml) else {
            pages.push(rel.clone());
            continue;
        };
        let Some(title) = fm.title else {
            pages.push(rel.clone());
            continue;
        };
        notes.push(NoteDraft {
            id,
            title,
            area: fm.area,
            status: fm.status,
            updated: fm.updated.as_ref().and_then(scalar_to_string),
            source_paths: fm.sources.iter().map(|s| s.path().to_string()).collect(),
            body: body.to_string(),
        });
    }

    if walk_truncated {
        findings.push(Finding {
            kind: FindingKind::Truncated,
            note: String::new(),
            detail: format!("stopped walking after {MAX_FILES} files"),
        });
    }

    // Every discovered file, note or page, keyed by its id (path without
    // `.md`) -- what `[[link]]` resolution rules 1 and 2 check against.
    let mut by_id: HashMap<String, FileKind> = HashMap::new();
    for n in &notes {
        by_id.insert(n.id.clone(), FileKind::Note);
    }
    for p in &pages {
        by_id.insert(p[..p.len() - 3].to_string(), FileKind::Page);
    }

    // Notes only, keyed by file stem -- rule 3.
    let mut by_stem: HashMap<String, Vec<String>> = HashMap::new();
    for n in &notes {
        let stem = n.id.rsplit('/').next().unwrap_or(&n.id).to_string();
        by_stem.entry(stem).or_default().push(n.id.clone());
    }

    let mut note_links: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut note_gaps: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut backlinks_acc: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut gaps_acc: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for draft in &notes {
        if draft.source_paths.is_empty() {
            findings.push(Finding {
                kind: FindingKind::Unsourced,
                note: draft.id.clone(),
                detail: "no sources in frontmatter".into(),
            });
        }
        for path in &draft.source_paths {
            // Decided from the string alone, before anything below ever
            // touches the filesystem -- see the doc comment on
            // `is_secret_source`.
            if is_secret_source(path) {
                findings.push(Finding {
                    kind: FindingKind::SecretSource,
                    note: draft.id.clone(),
                    detail: "a cited source lies under data/secrets/, which is never read".into(),
                });
                continue;
            }
            if eligible_for_stat(path) && fs::metadata(root.join(path)).is_err() {
                findings.push(Finding {
                    kind: FindingKind::MissingSource,
                    note: draft.id.clone(),
                    detail: format!("source not found: {}", truncate_for_detail(path)),
                });
            }
        }

        let mut missing = Vec::new();
        if draft.area.is_none() {
            missing.push("area");
        }
        if draft.status.is_none() {
            missing.push("status");
        }
        if draft.updated.is_none() {
            missing.push("updated");
        }
        if !missing.is_empty() {
            findings.push(Finding {
                kind: FindingKind::IncompleteFrontmatter,
                note: draft.id.clone(),
                detail: format!("missing {}", missing.join(", ")),
            });
        }

        let dir = dir_of(&draft.id);
        // De-duplicated per note before resolution, so writing the same
        // target twice (a repeated link, or the same target under two
        // labels) is one edge, not two.
        let raw_targets: BTreeSet<String> = extract_links(&draft.body).into_iter().collect();
        let links = note_links.entry(draft.id.clone()).or_default();
        let gaps = note_gaps.entry(draft.id.clone()).or_default();

        for target in raw_targets {
            match resolve(&target, dir, &by_id, &by_stem) {
                Resolution::Found(id, FileKind::Note) => {
                    if id == draft.id {
                        continue; // a self-link is ignored, not a gap
                    }
                    backlinks_acc
                        .entry(id.clone())
                        .or_default()
                        .insert(draft.id.clone());
                    links.insert(id);
                }
                Resolution::Found(_, FileKind::Page) => {
                    gaps.insert(target.clone());
                    gaps_acc.entry(target).or_default().insert(draft.id.clone());
                }
                Resolution::Ambiguous(mut candidates) => {
                    candidates.sort();
                    findings.push(Finding {
                        kind: FindingKind::AmbiguousLink,
                        note: draft.id.clone(),
                        detail: truncate_for_detail(&format!(
                            "[[{target}]] matches more than one note: {}",
                            candidates.join(", ")
                        ))
                        .into_owned(),
                    });
                    gaps.insert(target.clone());
                    gaps_acc.entry(target).or_default().insert(draft.id.clone());
                }
                Resolution::Unresolved => {
                    gaps.insert(target.clone());
                    gaps_acc.entry(target).or_default().insert(draft.id.clone());
                }
            }
        }
    }

    let mut final_notes: Vec<Note> = notes
        .into_iter()
        .map(|d| {
            let sources = d.source_paths.len();
            let links: Vec<String> = note_links
                .remove(&d.id)
                .unwrap_or_default()
                .into_iter()
                .collect();
            let gaps: Vec<String> = note_gaps
                .remove(&d.id)
                .unwrap_or_default()
                .into_iter()
                .collect();
            let backlinks: Vec<String> = backlinks_acc
                .remove(&d.id)
                .unwrap_or_default()
                .into_iter()
                .collect();
            if backlinks.is_empty() {
                findings.push(Finding {
                    kind: FindingKind::Orphan,
                    note: d.id.clone(),
                    detail: "no other note links here".into(),
                });
            }
            Note {
                id: d.id,
                title: d.title,
                area: d.area,
                status: d.status,
                updated: d.updated,
                sources,
                links,
                gaps,
                backlinks,
            }
        })
        .collect();
    final_notes.sort_by(|a, b| a.id.cmp(&b.id));

    let mut gaps: Vec<Gap> = gaps_acc
        .into_iter()
        .map(|(target, from)| Gap {
            target,
            from: from.into_iter().collect(),
        })
        .collect();
    gaps.sort_by(|a, b| {
        b.from
            .len()
            .cmp(&a.from.len())
            .then_with(|| a.target.cmp(&b.target))
    });

    pages.sort();

    // Kind, then note, then detail: the third key only ever breaks a tie
    // between two findings that share both, which otherwise leaves their
    // relative order to insertion sequence -- an implementation detail, not
    // something a byte-identical-output promise should depend on.
    findings.sort_by(|a, b| {
        a.kind
            .cmp(&b.kind)
            .then_with(|| a.note.cmp(&b.note))
            .then_with(|| a.detail.cmp(&b.detail))
    });
    // Two identical findings (e.g. two `sources[]` entries under
    // `data/secrets/` in the same note) carry no more information than one --
    // the sort above already puts every `(kind, note, detail)` duplicate
    // adjacent, so a single dedup pass is enough.
    findings.dedup_by(|a, b| a.kind == b.kind && a.note == b.note && a.detail == b.detail);

    Index {
        root: root_display,
        present: true,
        notes: final_notes,
        gaps,
        pages,
        findings,
    }
}

/// A note in progress: everything read off disk before links can be
/// resolved, which needs every other file's id known first.
struct NoteDraft {
    id: String,
    title: String,
    area: Option<String>,
    status: Option<String>,
    updated: Option<String>,
    source_paths: Vec<String>,
    /// The text after the closing `---`. Read only to extract `[[links]]`;
    /// it is never kept once `index` returns.
    body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Note,
    Page,
}

enum Resolution {
    Found(String, FileKind),
    Ambiguous(Vec<String>),
    Unresolved,
}

/// Frontmatter as YAML actually allows it to be written, before this module
/// decides what a note is. No `deny_unknown_fields`: an unrecognised key is
/// tolerated, not refused -- `SCHEMA.md` is a living document and a field it
/// adds tomorrow should not turn every existing note into a page today.
#[derive(Debug, Clone, Deserialize, Default)]
struct RawFrontMatter {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    area: Option<String>,
    #[serde(default)]
    status: Option<String>,
    /// Usually a string, but YAML's plain-scalar rules make an unquoted date
    /// (`updated: 2026-01-01`) ambiguous with other scalar types depending on
    /// the parser. Read as the raw value and rendered with `scalar_to_string`
    /// so any of them ends up as the string the payload carries.
    #[serde(default)]
    updated: Option<serde_yaml_ng::Value>,
    #[serde(default)]
    sources: Vec<RawSource>,
}

/// `sources:` entries as they actually appear: `SCHEMA.md` says
/// `{path, accessed}`, but a plain string (treated as the path, with no
/// access date) is also on disk. `accessed` and any other key a map carries
/// are read and discarded -- v1 counts sources and checks their paths, and
/// never needed to know when one was last looked at.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum RawSource {
    Path(String),
    Map { path: String },
}

impl RawSource {
    fn path(&self) -> &str {
        match self {
            RawSource::Path(p) => p,
            RawSource::Map { path } => path,
        }
    }
}

/// Render any YAML scalar as a string, the way `InterfaceConfig::string`
/// already does in `config.rs`: a plain string as itself, anything else
/// (a number, a bool, a date the parser read as something other than a
/// string) re-serialized and trimmed rather than dropped.
fn scalar_to_string(v: &serde_yaml_ng::Value) -> Option<String> {
    match v {
        serde_yaml_ng::Value::Null => None,
        serde_yaml_ng::Value::String(s) => Some(s.clone()),
        other => serde_yaml_ng::to_string(other)
            .ok()
            .map(|s| s.trim().to_string()),
    }
}

/// Split a file into its frontmatter YAML and its body. `None` when the file
/// does not start with a `---` line, or that line's block never closes --
/// either way, the file has no frontmatter this module can use, and it is
/// filed as a page rather than guessed at. A leading UTF-8 BOM (`\u{feff}`)
/// is stripped first: some editors write one, and it would otherwise sit in
/// front of the `---` and make every such file look frontmatter-less.
fn split_frontmatter(content: &str) -> Option<(&str, &str)> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let mut lines = content.split_inclusive('\n');
    let first = lines.next()?;
    if first.trim_end_matches(['\n', '\r']) != "---" {
        return None;
    }
    let rest = &content[first.len()..];
    let mut offset = 0usize;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']) == "---" {
            return Some((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

/// A link target longer than this is not a link -- it is prose that happened
/// to sit between two `[[`/`]]` pairs (or never closed until some distant,
/// unrelated `]]`). No real note id or bare file name comes close to this;
/// the cap exists purely so an unbounded span of body text can never reach
/// `gaps[].target`, `notes[].gaps` or a finding's `detail`.
const MAX_LINK_TARGET_LEN: usize = 200;

/// Whether `target` (already stripped of a `|label`/`#heading` suffix and
/// trimmed) is shaped like something a person actually wrote as a wiki link,
/// as opposed to text that merely landed between two `[[`/`]]` delimiters --
/// a 5,000-character paragraph, or a bash `[[ -f x ]]` test caught by the
/// same bracket pair. A real target (`some-page`, `partners/acme`, `person`)
/// never contains whitespace: every documented and observed resolution rule
/// works on `/`-separated path segments, none of which has room for a space.
/// Rejecting whitespace is a generalisation of the `\n`/`\r` rule below, not
/// a departure from it -- and it is the only rule that also catches short,
/// space-containing prose (`[[ see the pricing page ]]`) that a length cap
/// alone would let through.
fn is_link_target_shape(target: &str) -> bool {
    if target.is_empty() || target.ends_with('/') {
        return false;
    }
    if target.chars().count() > MAX_LINK_TARGET_LEN {
        return false;
    }
    !target.chars().any(|c| c == '[' || c == ']' || c.is_whitespace())
}

/// Every `[[...]]` in `body`, stripped of a `|label` or `#heading` suffix and
/// trimmed. A span that is not shaped like a real link target -- see
/// `is_link_target_shape` -- is not a link at all: it is skipped and never
/// reported anywhere, rather than resolved or listed as a gap. Scanning
/// always resumes from just past the `]]` that closed the span under
/// consideration, whether or not that span turned out to be a link, so a
/// rejected `[[...]]` never swallows a valid one later on the same line.
fn extract_links(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel_start) = body[cursor..].find("[[") {
        let start = cursor + rel_start + 2;
        let Some(rel_end) = body[start..].find("]]") else {
            break;
        };
        let end = start + rel_end;
        let raw = &body[start..end];
        // Resume from here regardless of what the validity checks below
        // decide -- a rejected span must not carry the cursor backwards or
        // leave it stuck, or a later valid link on the same line would never
        // be found.
        cursor = end + 2;
        if raw.contains(['\n', '\r']) {
            continue;
        }
        let cut = raw.find(['|', '#']);
        let target = match cut {
            Some(p) => &raw[..p],
            None => raw,
        };
        let target = target.trim();
        if !is_link_target_shape(target) {
            continue;
        }
        out.push(target.to_string());
    }
    out
}

/// The wiki-root-relative directory a note lives in -- `""` for a note at
/// the wiki root, the parent path otherwise.
fn dir_of(id: &str) -> &str {
    match id.rfind('/') {
        Some(i) => &id[..i],
        None => "",
    }
}

/// Join `base_dir` (a wiki-root-relative directory, `/`-separated) and
/// `target` (a link target, also `/`-separated) and resolve `.`/`..`
/// components without touching the filesystem. `None` when the result would
/// climb above the wiki root, or when it normalises to nothing at all --
/// either way, this resolution rule simply does not match, and the caller
/// moves on to the next one.
fn normalize_relative(base_dir: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = if base_dir.is_empty() {
        Vec::new()
    } else {
        base_dir.split('/').collect()
    };
    for comp in target.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// The three resolution rules, tried in order, stopping at the first match:
/// page-relative, then root-relative, then a unique note file stem. See the
/// module doc comment for where this is written up.
fn resolve(
    target: &str,
    dir: &str,
    by_id: &HashMap<String, FileKind>,
    by_stem: &HashMap<String, Vec<String>>,
) -> Resolution {
    if let Some(id) = normalize_relative(dir, target) {
        if let Some(kind) = by_id.get(&id) {
            return Resolution::Found(id, *kind);
        }
    }
    if let Some(id) = normalize_relative("", target) {
        if let Some(kind) = by_id.get(&id) {
            return Resolution::Found(id, *kind);
        }
    }
    let last = target.rsplit('/').next().unwrap_or(target);
    if let Some(candidates) = by_stem.get(last) {
        return match candidates.len() {
            1 => Resolution::Found(candidates[0].clone(), FileKind::Note),
            _ => Resolution::Ambiguous(candidates.clone()),
        };
    }
    Resolution::Unresolved
}

/// Bound how much of a user-controlled string (a link target that already
/// passed `is_link_target_shape`, but also a `sources[].path` that has no
/// such cap at all) a finding's `detail` may embed. A rejected link target
/// never reaches here, but nothing stops a `sources:` entry in frontmatter
/// from being an arbitrarily long string, and `detail` is serialized
/// verbatim -- so this is the backstop for that path, not a duplicate of
/// `MAX_LINK_TARGET_LEN`.
const MAX_DETAIL_EMBED_LEN: usize = 200;

fn truncate_for_detail(s: &str) -> std::borrow::Cow<'_, str> {
    if s.chars().count() <= MAX_DETAIL_EMBED_LEN {
        return std::borrow::Cow::Borrowed(s);
    }
    let head: String = s.chars().take(MAX_DETAIL_EMBED_LEN).collect();
    std::borrow::Cow::Owned(format!("{head}…"))
}

/// Whether `path` (a `sources[].path` exactly as written in frontmatter) lies
/// under `data/secrets/`, compared case-insensitively with `.` components
/// dropped. **This is a string comparison and nothing else** -- it runs
/// before any other source check, and its result decides whether this
/// module ever calls `fs::metadata` on the path at all. A directory `chmod
/// 000` to prove that is exactly what one of this module's tests does.
fn is_secret_source(path: &str) -> bool {
    let comps: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    comps.len() >= 2
        && comps[0].eq_ignore_ascii_case("data")
        && comps[1].eq_ignore_ascii_case("secrets")
}

/// Whether `path` is the kind of `sources[].path` `missing_source` may stat:
/// relative, with no `..` component, and not a URL. Absolute paths and URLs
/// are silently out of scope -- v1 only ever asks the instance's own
/// filesystem, and only ever underneath it.
fn eligible_for_stat(path: &str) -> bool {
    if path.contains("://") {
        return false;
    }
    let p = Path::new(path);
    if p.is_absolute() {
        return false;
    }
    !p.components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// Every `.md` file under `wiki_root`, recursively, skipping dotfiles and
/// dot-directories and never following a symlink. Stops at `MAX_FILES` and
/// says so in the returned bool, rather than reading an unbounded tree. The
/// stop is immediate: hitting the cap returns right away rather than letting
/// the outer walk keep popping and `read_dir`-ing queued directories it will
/// never use the contents of.
fn discover(wiki_root: &Path) -> (Vec<(String, PathBuf)>, bool) {
    let mut out = Vec::new();
    let mut stack = vec![(wiki_root.to_path_buf(), String::new())];
    while let Some((dir, rel_dir)) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        // Sorted so that which files land on the right side of the cap is
        // stable for a given tree, rather than however the OS happened to
        // hand back directory entries.
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name();
            let name_str = name.to_string_lossy().to_string();
            if name_str.starts_with('.') {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let rel = if rel_dir.is_empty() {
                name_str.clone()
            } else {
                format!("{rel_dir}/{name_str}")
            };
            if file_type.is_dir() {
                stack.push((entry.path(), rel));
            } else if file_type.is_file() && name_str.to_ascii_lowercase().ends_with(".md") {
                if out.len() >= MAX_FILES {
                    return (out, true);
                }
                out.push((rel, entry.path()));
            }
        }
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_wiki(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "factory-knowledge-test-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(dir.join("knowledge/wiki")).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join("knowledge/wiki").join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn note_by_id<'a>(idx: &'a Index, id: &str) -> &'a Note {
        idx.notes
            .iter()
            .find(|n| n.id == id)
            .unwrap_or_else(|| panic!("no note {id}"))
    }

    #[test]
    fn page_relative_root_relative_and_bare_name_resolution_all_work() {
        let root = temp_wiki("resolution");
        write(
            &root,
            "partners/acme.md",
            "---\ntitle: Acme Co\narea: partners\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: partners/acme-note.md\n---\nSee [[jane-doe]] and [[roadmap]].\n",
        );
        write(
            &root,
            "partners/jane-doe.md",
            "---\ntitle: Jane Doe\narea: partners\nstatus: current\nupdated: 2026-01-02\nsources:\n  - path: partners/jane-doe-note.md\n---\nContact for [[partners/acme]].\n",
        );
        write(
            &root,
            "gadgets/roadmap.md",
            "---\ntitle: Gadget Roadmap\narea: gadgets\nstatus: current\nupdated: 2026-01-03\nsources:\n  - path: gadgets/roadmap-note.md\n---\nNothing links out of here.\n",
        );

        let idx = index(&root);
        assert!(idx.present);
        assert_eq!(idx.notes.len(), 3);

        let acme = note_by_id(&idx, "partners/acme");
        assert!(
            acme.links.contains(&"partners/jane-doe".to_string()),
            "page-relative: {:?}",
            acme.links
        );
        assert!(
            acme.links.contains(&"gadgets/roadmap".to_string()),
            "bare-name: {:?}",
            acme.links
        );

        let jane = note_by_id(&idx, "partners/jane-doe");
        assert!(
            jane.links.contains(&"partners/acme".to_string()),
            "root-relative: {:?}",
            jane.links
        );

        let roadmap = note_by_id(&idx, "gadgets/roadmap");
        assert!(roadmap.backlinks.contains(&"partners/acme".to_string()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_ambiguous_file_name_is_reported_and_left_unresolved() {
        let root = temp_wiki("ambiguous");
        write(
            &root,
            "clients/contact.md",
            "---\ntitle: Clients Contact\narea: clients\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: clients/contact-note.md\n---\nNo links.\n",
        );
        write(
            &root,
            "ops/contact.md",
            "---\ntitle: Ops Contact\narea: operations\nstatus: current\nupdated: 2026-01-02\nsources:\n  - path: ops/contact-note.md\n---\nNo links.\n",
        );
        // Neither the linking note's own directory nor the wiki root has a
        // `contact.md`, so rules 1 and 2 both miss and resolution falls
        // through to rule 3 -- where the ambiguity actually lives. Putting
        // the link in `clients/` or `ops/` instead would resolve it via rule
        // 1 before rule 3 is ever tried, which is not the case this proves.
        write(
            &root,
            "general/other.md",
            "---\ntitle: General Notes\narea: general\nstatus: current\nupdated: 2026-01-03\nsources:\n  - path: general/other-note.md\n---\nTalk to [[contact]] about it.\n",
        );

        let idx = index(&root);
        let other = note_by_id(&idx, "general/other");
        assert!(
            other.links.is_empty(),
            "an ambiguous name never resolves: {:?}",
            other.links
        );
        assert!(other.gaps.contains(&"contact".to_string()));
        assert!(
            idx.findings
                .iter()
                .any(|f| f.kind == FindingKind::AmbiguousLink && f.note == "general/other"),
            "{:?}",
            idx.findings
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unwritten_target_is_a_gap_listing_every_note_that_points_at_it() {
        let root = temp_wiki("gap-pointers");
        write(
            &root,
            "a.md",
            "---\ntitle: A\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: a-note.md\n---\nSee [[phantom]].\n",
        );
        write(
            &root,
            "b.md",
            "---\ntitle: B\narea: x\nstatus: current\nupdated: 2026-01-02\nsources:\n  - path: b-note.md\n---\nAlso see [[phantom]], and [[lonely-gap]].\n",
        );

        let idx = index(&root);
        let phantom = idx
            .gaps
            .iter()
            .find(|g| g.target == "phantom")
            .expect("phantom gap");
        assert_eq!(phantom.from, vec!["a".to_string(), "b".to_string()]);

        let lonely = idx
            .gaps
            .iter()
            .find(|g| g.target == "lonely-gap")
            .expect("lonely gap");
        assert_eq!(lonely.from, vec!["b".to_string()]);

        // Sorted by descending pointer count, then by target.
        let phantom_pos = idx.gaps.iter().position(|g| g.target == "phantom").unwrap();
        let lonely_pos = idx
            .gaps
            .iter()
            .position(|g| g.target == "lonely-gap")
            .unwrap();
        assert!(
            phantom_pos < lonely_pos,
            "two pointers sorts before one: {:?}",
            idx.gaps
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_page_without_frontmatter_is_not_a_node_and_its_links_count_for_nothing() {
        let root = temp_wiki("page-not-a-node");
        write(
            &root,
            "index.md",
            "# Index\n\nSee [[some-note]] and [[another]].\n",
        );
        write(
            &root,
            "some-note.md",
            "---\ntitle: Some Note\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: some-note-source.md\n---\nNo links out.\n",
        );

        let idx = index(&root);
        assert_eq!(idx.notes.len(), 1, "index.md is not a note");
        assert!(idx.pages.contains(&"index.md".to_string()));
        assert!(
            idx.gaps
                .iter()
                .all(|g| g.target != "some-note" && g.target != "another"),
            "a page's links are not counted at all, not even as gaps: {:?}",
            idx.gaps
        );
        let some_note = note_by_id(&idx, "some-note");
        assert!(
            some_note.backlinks.is_empty(),
            "the page's link does not count as a backlink either"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_link_that_resolves_to_a_page_is_a_gap() {
        let root = temp_wiki("page-as-target");
        write(&root, "log.md", "# Log\n\nAppend-only.\n");
        write(
            &root,
            "a.md",
            "---\ntitle: A\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: a-note.md\n---\nSee [[log]] for history.\n",
        );

        let idx = index(&root);
        let a = note_by_id(&idx, "a");
        assert!(a.links.is_empty());
        assert!(a.gaps.contains(&"log".to_string()));
        assert!(idx
            .gaps
            .iter()
            .any(|g| g.target == "log" && g.from == vec!["a".to_string()]));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn every_finding_kind_fires() {
        let root = temp_wiki("every-finding");
        // unsourced, orphan, incomplete_frontmatter (missing status/updated)
        write(
            &root,
            "unsourced.md",
            "---\ntitle: Unsourced\narea: x\n---\nNothing to see.\n",
        );
        // secret_source, alongside a real (missing) source
        write(
            &root,
            "secretive.md",
            "---\ntitle: Secretive\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: data/secrets/creds.md\n  - path: nowhere/near-here.md\n---\nNo links.\n",
        );
        // ambiguous_link, via two same-stemmed notes
        write(
            &root,
            "clients/dup.md",
            "---\ntitle: Client Dup\narea: clients\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: clients/dup-note.md\n---\nNo links.\n",
        );
        write(
            &root,
            "ops/dup.md",
            "---\ntitle: Ops Dup\narea: ops\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: ops/dup-note.md\n---\nNo links.\n",
        );
        write(
            &root,
            "linker.md",
            "---\ntitle: Linker\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: linker-note.md\n---\nSee [[dup]].\n",
        );
        // truncated, via an oversized file
        let huge = "x".repeat((MAX_FILE_BYTES + 1) as usize);
        write(&root, "huge.md", &huge);

        let idx = index(&root);
        let kinds: BTreeSet<FindingKind> = idx.findings.iter().map(|f| f.kind).collect();
        for expected in [
            FindingKind::Unsourced,
            FindingKind::SecretSource,
            FindingKind::MissingSource,
            FindingKind::IncompleteFrontmatter,
            FindingKind::Orphan,
            FindingKind::AmbiguousLink,
            FindingKind::Truncated,
        ] {
            assert!(
                kinds.contains(&expected),
                "missing {expected:?} in {:?}",
                idx.findings
            );
        }
        assert!(
            idx.pages.contains(&"huge.md".to_string()),
            "an oversized file is filed as a page"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The literal proof the issue asks for: a source under `data/secrets/`
    /// is reported without ever being opened or stat-ed. `chmod 000` on the
    /// directory means any attempt to reach through it -- even a stat of a
    /// file inside -- fails; if `missing_source` fired for this path, that
    /// would mean the code tried and hit the permission error, which is
    /// exactly what must never happen.
    #[test]
    fn a_source_under_data_secrets_is_flagged_without_ever_being_read() {
        let root = temp_wiki("secrets-proof");
        let secrets_dir = root.join("data/secrets");
        std::fs::create_dir_all(&secrets_dir).unwrap();
        std::fs::write(
            secrets_dir.join("x.md"),
            "sentinel-secret-body-should-never-leak",
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&secrets_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        }

        write(
            &root,
            "citer.md",
            "---\ntitle: Citer\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: data/secrets/x.md\n---\nNo links.\n",
        );

        let idx = index(&root);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&secrets_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        assert!(
            idx.findings
                .iter()
                .any(|f| f.kind == FindingKind::SecretSource && f.note == "citer"),
            "{:?}",
            idx.findings
        );
        assert!(
            !idx.findings
                .iter()
                .any(|f| f.kind == FindingKind::MissingSource && f.note == "citer"),
            "a stat through a 000 directory would fail, so this proves nothing was stat-ed: {:?}",
            idx.findings
        );

        let json = serde_json::to_string(&idx).unwrap();
        assert!(
            !json.contains("secrets/x"),
            "the path never appears anywhere in the payload"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_note_body_text_ever_reaches_the_payload() {
        let root = temp_wiki("no-body-leak");
        // The sentinel sits inside a `[[...]]` span, with a newline in the
        // middle of it -- exactly the shape `extract_links` must reject
        // (finding 1), so this proves the rejection, not just that ordinary
        // prose outside any `[[...]]` was never a link candidate to begin
        // with.
        write(
            &root,
            "quiet.md",
            "---\ntitle: Quiet\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: quiet-note.md\n---\nSee [[SENTINEL-BODY-TEXT-f8a2c1\nshould never be serialized]].\n",
        );

        let idx = index(&root);
        let json = serde_json::to_string(&idx).unwrap();
        assert!(!json.contains("SENTINEL-BODY-TEXT-f8a2c1"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_five_thousand_char_span_between_double_brackets_never_appears_in_the_index() {
        let root = temp_wiki("huge-link-span");
        let prose = "x".repeat(5_000);
        write(
            &root,
            "a.md",
            &format!("---\ntitle: A\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: a-note.md\n---\nSee [[{prose}]] for more.\n"),
        );

        let idx = index(&root);
        let json = serde_json::to_string(&idx).unwrap();
        assert!(!json.contains(&prose), "the 5,000-char span must never be serialized");
        assert!(
            idx.gaps.iter().all(|g| g.target.len() < 5_000),
            "the oversized span must not become a gap target: {:?}",
            idx.gaps
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bash_double_bracket_test_syntax_is_not_treated_as_a_link() {
        let root = temp_wiki("bash-brackets");
        write(
            &root,
            "a.md",
            "---\ntitle: A\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: a-note.md\n---\n```bash\nif [[ -f x ]]; then\n  echo hi\nfi\n```\n",
        );

        let idx = index(&root);
        assert!(
            idx.gaps.iter().all(|g| !g.target.contains("-f")),
            "a bash [[ -f x ]] test must never become a gap: {:?}",
            idx.gaps
        );
        let a = note_by_id(&idx, "a");
        assert!(a.gaps.is_empty(), "{:?}", a.gaps);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_valid_link_after_a_rejected_one_on_the_same_line_still_resolves() {
        let root = temp_wiki("resume-after-reject");
        write(
            &root,
            "real-note.md",
            "---\ntitle: Real Note\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: real-note-source.md\n---\nNo links out.\n",
        );
        write(
            &root,
            "linker.md",
            "---\ntitle: Linker\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: linker-note.md\n---\nx [[ -f y ]] and [[real-note]]\n",
        );

        let idx = index(&root);
        let linker = note_by_id(&idx, "linker");
        assert_eq!(
            linker.links,
            vec!["real-note".to_string()],
            "the rejected [[ -f y ]] must not block the valid link after it: {linker:?}"
        );
        assert!(linker.gaps.is_empty(), "{:?}", linker.gaps);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_wiki_directory_is_an_empty_state_not_an_error() {
        let root = std::env::temp_dir().join(format!(
            "factory-knowledge-test-missing-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();

        let idx = index(&root);
        assert!(!idx.present);
        assert!(idx.notes.is_empty());
        assert!(idx.root.ends_with("knowledge/wiki") || idx.root.ends_with("knowledge\\wiki"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_leading_utf8_bom_does_not_defeat_frontmatter_detection() {
        let root = temp_wiki("bom");
        write(
            &root,
            "bommed.md",
            "\u{feff}---\ntitle: Bommed\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: bommed-note.md\n---\nNo links.\n",
        );

        let idx = index(&root);
        assert_eq!(idx.notes.len(), 1, "a BOM must not turn a note into a page");
        assert!(idx.pages.is_empty(), "{:?}", idx.pages);
        let n = note_by_id(&idx, "bommed");
        assert_eq!(n.title, "Bommed");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn discover_stops_immediately_once_the_file_cap_is_hit() {
        let root = temp_wiki("discover-cap");
        // Well over MAX_FILES, spread across two sibling directories so the
        // walk has more than one directory queued when the cap is hit.
        for i in 0..(MAX_FILES + 50) {
            let dir = if i % 2 == 0 { "a" } else { "b" };
            write(&root, &format!("{dir}/n{i:05}.md"), "no frontmatter\n");
        }

        let (files, truncated) = discover(&root.join("knowledge/wiki"));
        assert!(truncated, "hitting the cap must still report truncation");
        assert_eq!(
            files.len(),
            MAX_FILES,
            "the walk must stop exactly at the cap rather than collecting more"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn duplicate_findings_with_the_same_kind_note_and_detail_are_collapsed_to_one() {
        let root = temp_wiki("dedup-findings");
        write(
            &root,
            "citer.md",
            "---\ntitle: Citer\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: data/secrets/a.md\n  - path: data/secrets/b.md\n---\nNo links.\n",
        );

        let idx = index(&root);
        let secret_findings: Vec<_> = idx
            .findings
            .iter()
            .filter(|f| f.kind == FindingKind::SecretSource && f.note == "citer")
            .collect();
        assert_eq!(
            secret_findings.len(),
            1,
            "two data/secrets/ citations in one note must collapse to one identical finding: {:?}",
            idx.findings
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_source_detail_embeds_a_bounded_length_path() {
        let root = temp_wiki("long-source-path");
        let long_path = format!("nowhere/{}.md", "y".repeat(1_000));
        write(
            &root,
            "a.md",
            &format!("---\ntitle: A\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: {long_path}\n---\nNo links.\n"),
        );

        let idx = index(&root);
        let finding = idx
            .findings
            .iter()
            .find(|f| f.kind == FindingKind::MissingSource && f.note == "a")
            .expect("missing_source finding");
        assert!(
            finding.detail.len() < long_path.len(),
            "the embedded path must be bounded: {} chars",
            finding.detail.len()
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_same_files_always_produce_the_same_bytes() {
        let root = temp_wiki("determinism");
        write(
            &root,
            "a.md",
            "---\ntitle: A\narea: x\nstatus: current\nupdated: 2026-01-01\nsources:\n  - path: a-note.md\n---\nSee [[b]] and [[missing]].\n",
        );
        write(
            &root,
            "b.md",
            "---\ntitle: B\narea: x\nstatus: current\nupdated: 2026-01-02\nsources: []\n---\nSee [[a]].\n",
        );

        let first = serde_json::to_string(&index(&root)).unwrap();
        let second = serde_json::to_string(&index(&root)).unwrap();
        assert_eq!(first, second);

        std::fs::remove_dir_all(&root).ok();
    }
}
