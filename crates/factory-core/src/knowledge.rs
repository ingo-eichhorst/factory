//! A read-only index of the instance's knowledge vault, rebuilt from the
//! files on every request, plus the pure planning behind the only way
//! anything is ever written to it. There is no link table to keep in step:
//! `index` walks `<root>/.factory/knowledge/` and returns titles, frontmatter
//! fields, tags, links, document references and findings. **No page body
//! text and no document bytes ever appear in the result** -- a reader who
//! wants either opens the file itself.
//!
//! `index` is a pure function of what is on disk: the same files always
//! produce the same bytes. It is meant to run inside `spawn_blocking` (see
//! `factory-daemon/src/engine.rs`) because a large vault is a filesystem
//! walk, not something to do on the async runtime's own thread.
//!
//! Writing is the other half. `import` and `add` copy files in -- never edit,
//! never delete -- and refuse a target that escapes the vault or a source
//! that reads from somewhere it should not, deciding the latter from the path
//! string alone, before a single byte is touched.
//!
//! This is v2 of the module the issue's Part 1 describes; v1 read
//! `<root>/knowledge/wiki/` and split `.md` files into "notes" (frontmatter
//! with a title) and "pages" (everything else, not a graph node). v2 makes
//! every `.md` file a page and a graph node, and that vault lives under
//! `.factory/`, which nothing else in Factory owns or regenerates. The old
//! path is read only to report as `legacy` when the vault itself is missing.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

// ================================================================== the index

/// No vault this large has been tried. The cap exists so a request against
/// one that is stops in bounded time instead of walking forever; hitting it
/// is a `truncated` finding, never an error. v1's limit, carried over.
const MAX_PAGES: usize = 2_000;

/// A page over this size is not read at all -- its frontmatter, tags and
/// links are unknown, and its title falls back to the file name rather than
/// being guessed at. v1's limit, carried over. 1 MiB.
const MAX_PAGE_BYTES: u64 = 1024 * 1024;

/// Every other regular file gets a much larger allowance, because a document
/// is stat-ed and never opened -- there is no parsing cost to bound.
const MAX_DOCUMENTS: usize = 10_000;

/// One `.md` file: a graph node always, whether or not it carries
/// frontmatter. Its id is its path relative to the vault root, `/`-separated,
/// without `.md`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Page {
    pub id: String,
    /// The frontmatter `title`, or the file name (without `.md`) when there
    /// is none -- either because the file carries no frontmatter at all, or
    /// because its frontmatter has no `title` field.
    pub title: String,
    /// Whether frontmatter parsed at all. A page can have this `true` and
    /// still fall back to the file name for `title`, if the block it wrote
    /// never names one.
    pub frontmatter: bool,
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
    /// Resolved page ids this page points at, deduplicated and sorted.
    pub links: Vec<String>,
    /// Targets this page points at that never resolved to a page -- as
    /// written, after stripping `|label`/`#heading` -- deduplicated and
    /// sorted. Shared with document gaps: a target that named a document and
    /// missed lands here too.
    pub gaps: Vec<String>,
    /// Every page whose own `links` resolves here, sorted.
    pub backlinks: Vec<String>,
    /// Every tag this page carries -- frontmatter `tags`/`keywords` and
    /// inline `#tag`, lowercased, deduplicated and sorted.
    pub tags: Vec<String>,
    /// Resolved document ids this page references, sorted.
    pub documents: Vec<String>,
}

/// One tag node: every tag written anywhere in the vault, and the pages that
/// carry it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub pages: Vec<String>,
}

/// One document node: any regular file under the vault that is not `.md`.
/// Stat-ed, never opened -- `bytes` is the only fact about its content this
/// module ever learns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Document {
    pub id: String,
    /// Lowercased, without the dot. Empty when the file name has none.
    pub ext: String,
    pub bytes: u64,
    pub referenced_by: Vec<String>,
}

/// A target written somewhere in the vault that nothing answers: a page id or
/// a document path that never resolved, or a name that matched more than one
/// candidate.
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
    /// The page the finding is about -- named `note` on the wire, kept from
    /// v1 rather than renamed, since the issue's own wire example still
    /// spells it that way. Empty for a `truncated` finding that belongs to
    /// the walk as a whole rather than one file; a `truncated` finding for
    /// one oversized page names its path here instead.
    pub note: String,
    pub detail: String,
}

/// The whole answer to `GET /api/knowledge`, minus the `"kind"` tag that
/// `Payload::Knowledge` adds on the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Index {
    /// The absolute path this index was built from: `<root>/.factory/knowledge`.
    pub root: String,
    /// `false` when the vault does not exist. Not an error -- an instance
    /// with nothing added yet is an empty state.
    pub present: bool,
    /// `<root>/knowledge/wiki`, when `present` is `false` and that v1 path
    /// exists. Never set otherwise -- checking it when the vault is already
    /// there would make the index depend on a directory nothing reads from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy: Option<String>,
    pub pages: Vec<Page>,
    pub tags: Vec<Tag>,
    pub documents: Vec<Document>,
    /// Sorted by descending pointer count, then by target.
    pub gaps: Vec<Gap>,
    pub findings: Vec<Finding>,
}

/// `<root>/.factory/knowledge`, the vault.
pub fn vault_root(root: &Path) -> PathBuf {
    root.join(".factory").join("knowledge")
}

/// `<root>/knowledge/wiki`, v1's fixed path. Read only to name as `legacy`.
fn legacy_wiki_root(root: &Path) -> PathBuf {
    root.join("knowledge").join("wiki")
}

/// Walk the vault and build the index. Never fails: a missing vault is
/// `present: false`, and a walk limit hit is a `truncated` finding rather
/// than an error.
pub fn index(root: &Path) -> Index {
    let vault = vault_root(root);
    let root_display = vault.display().to_string();
    if !vault.is_dir() {
        let legacy = legacy_wiki_root(root);
        return Index {
            root: root_display,
            present: false,
            legacy: legacy.is_dir().then(|| legacy.display().to_string()),
            pages: Vec::new(),
            tags: Vec::new(),
            documents: Vec::new(),
            gaps: Vec::new(),
            findings: Vec::new(),
        };
    }

    let walk = discover(&vault);
    let mut findings: Vec<Finding> = Vec::new();

    // -- first pass: read every page, without resolving anything yet -------
    let mut pages: Vec<PageDraft> = Vec::new();
    for (rel, abs) in &walk.pages {
        pages.push(read_page(rel, abs, &mut findings));
    }
    if walk.pages_truncated {
        findings.push(Finding {
            kind: FindingKind::Truncated,
            note: String::new(),
            detail: format!("stopped indexing pages after {MAX_PAGES} files"),
        });
    }

    // -- documents: stat-ed only, never opened -------------------------------
    let mut documents: Vec<DocumentDraft> = Vec::new();
    for (rel, abs) in &walk.documents {
        let bytes = fs::metadata(abs).map(|m| m.len()).unwrap_or(0);
        documents.push(DocumentDraft {
            id: rel.clone(),
            ext: extension_of(rel).unwrap_or_default().to_ascii_lowercase(),
            bytes,
        });
    }
    if walk.documents_truncated {
        findings.push(Finding {
            kind: FindingKind::Truncated,
            note: String::new(),
            detail: format!("stopped indexing documents after {MAX_DOCUMENTS} files"),
        });
    }

    // Every page id, and the same keyed by file stem -- rules 1/2 and rule 3
    // of link resolution, respectively.
    let page_ids: HashSet<String> = pages.iter().map(|p| p.id.clone()).collect();
    let mut page_stems: HashMap<String, Vec<String>> = HashMap::new();
    for p in &pages {
        let stem = p.id.rsplit('/').next().unwrap_or(&p.id).to_string();
        page_stems.entry(stem).or_default().push(p.id.clone());
    }
    let doc_ids: HashSet<String> = documents.iter().map(|d| d.id.clone()).collect();
    let mut doc_stems: HashMap<String, Vec<String>> = HashMap::new();
    for d in &documents {
        let stem = d.id.rsplit('/').next().unwrap_or(&d.id).to_string();
        doc_stems.entry(stem).or_default().push(d.id.clone());
    }

    // -- second pass: resolve every page's links, tags and document refs ----
    let mut page_links: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut page_gaps: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut page_docs: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut backlinks_acc: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut doc_refs_acc: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut gaps_acc: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut tags_acc: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for draft in &pages {
        if draft.has_titled_frontmatter && draft.source_paths.is_empty() {
            findings.push(Finding {
                kind: FindingKind::Unsourced,
                note: draft.id.clone(),
                detail: "no sources in frontmatter".into(),
            });
        }
        for path in &draft.source_paths {
            // Decided from the string alone, before anything below ever
            // touches the filesystem -- see the doc comment on
            // `is_secret_source_field`.
            if is_secret_source_field(path) {
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

        if draft.has_titled_frontmatter {
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
        }

        for tag in &draft.tags {
            tags_acc.entry(tag.clone()).or_default().insert(draft.id.clone());
        }

        let dir = dir_of(&draft.id);
        let links = page_links.entry(draft.id.clone()).or_default();
        let gaps = page_gaps.entry(draft.id.clone()).or_default();
        let docs = page_docs.entry(draft.id.clone()).or_default();

        // Wiki-style (`[[x]]`/`![[x]]`) and Markdown (`[t](x)`) targets are
        // gathered and deduplicated separately, then resolved the same way --
        // whichever syntax a target arrived in, the same three-rule cascade
        // decides where it points.
        let mut raw_targets: BTreeSet<String> = extract_wiki_links(&draft.body).into_iter().collect();
        raw_targets.extend(extract_markdown_links(&draft.body));

        for raw in raw_targets {
            match classify_link(&raw) {
                LinkClass::Page(lookup) => {
                    match resolve(&lookup, dir, &page_ids, &page_stems) {
                        Resolution::Found(id) => {
                            if id == draft.id {
                                continue; // a self-link is ignored, not a gap
                            }
                            backlinks_acc.entry(id.clone()).or_default().insert(draft.id.clone());
                            links.insert(id);
                        }
                        Resolution::Ambiguous(mut candidates) => {
                            candidates.sort();
                            findings.push(Finding {
                                kind: FindingKind::AmbiguousLink,
                                note: draft.id.clone(),
                                detail: truncate_for_detail(&format!(
                                    "[[{raw}]] matches more than one page: {}",
                                    candidates.join(", ")
                                ))
                                .into_owned(),
                            });
                            gaps.insert(raw.clone());
                            gaps_acc.entry(raw).or_default().insert(draft.id.clone());
                        }
                        Resolution::Unresolved => {
                            gaps.insert(raw.clone());
                            gaps_acc.entry(raw).or_default().insert(draft.id.clone());
                        }
                    }
                }
                LinkClass::Document(lookup) => {
                    match resolve(&lookup, dir, &doc_ids, &doc_stems) {
                        Resolution::Found(id) => {
                            doc_refs_acc.entry(id.clone()).or_default().insert(draft.id.clone());
                            docs.insert(id);
                        }
                        Resolution::Ambiguous(mut candidates) => {
                            candidates.sort();
                            findings.push(Finding {
                                kind: FindingKind::AmbiguousLink,
                                note: draft.id.clone(),
                                detail: truncate_for_detail(&format!(
                                    "[[{raw}]] matches more than one document: {}",
                                    candidates.join(", ")
                                ))
                                .into_owned(),
                            });
                            gaps.insert(raw.clone());
                            gaps_acc.entry(raw).or_default().insert(draft.id.clone());
                        }
                        Resolution::Unresolved => {
                            gaps.insert(raw.clone());
                            gaps_acc.entry(raw).or_default().insert(draft.id.clone());
                        }
                    }
                }
            }
        }
    }

    let mut final_pages: Vec<Page> = pages
        .into_iter()
        .map(|d| {
            let links: Vec<String> = page_links.remove(&d.id).unwrap_or_default().into_iter().collect();
            let gaps: Vec<String> = page_gaps.remove(&d.id).unwrap_or_default().into_iter().collect();
            let documents: Vec<String> = page_docs.remove(&d.id).unwrap_or_default().into_iter().collect();
            let backlinks: Vec<String> = backlinks_acc.remove(&d.id).unwrap_or_default().into_iter().collect();
            // "Nothing connects to it": no page links here, it carries no
            // tag, and it references no document. An outgoing link to
            // somewhere else does not by itself save a page from this --
            // orphan is about being reachable from the rest of the graph,
            // not about what the page itself points at.
            if backlinks.is_empty() && d.tags.is_empty() && documents.is_empty() {
                findings.push(Finding {
                    kind: FindingKind::Orphan,
                    note: d.id.clone(),
                    detail: "no page, tag or document connects to it".into(),
                });
            }
            Page {
                id: d.id,
                title: d.title,
                frontmatter: d.frontmatter,
                area: d.area,
                status: d.status,
                updated: d.updated,
                sources: d.source_paths.len(),
                links,
                gaps,
                backlinks,
                tags: d.tags.into_iter().collect(),
                documents,
            }
        })
        .collect();
    final_pages.sort_by(|a, b| a.id.cmp(&b.id));

    let mut final_documents: Vec<Document> = documents
        .into_iter()
        .map(|d| Document {
            referenced_by: doc_refs_acc.remove(&d.id).unwrap_or_default().into_iter().collect(),
            id: d.id,
            ext: d.ext,
            bytes: d.bytes,
        })
        .collect();
    final_documents.sort_by(|a, b| a.id.cmp(&b.id));

    let final_tags: Vec<Tag> = tags_acc
        .into_iter()
        .map(|(name, pages)| Tag {
            name,
            pages: pages.into_iter().collect(),
        })
        .collect(); // already sorted: `tags_acc` is a `BTreeMap`

    let mut gaps: Vec<Gap> = gaps_acc
        .into_iter()
        .map(|(target, from)| Gap {
            target,
            from: from.into_iter().collect(),
        })
        .collect();
    gaps.sort_by(|a, b| b.from.len().cmp(&a.from.len()).then_with(|| a.target.cmp(&b.target)));

    // Kind, then note, then detail: the third key only ever breaks a tie
    // between two findings that share both, which otherwise leaves their
    // relative order to insertion sequence -- an implementation detail, not
    // something a byte-identical-output promise should depend on.
    findings.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| a.note.cmp(&b.note)).then_with(|| a.detail.cmp(&b.detail)));
    findings.dedup_by(|a, b| a.kind == b.kind && a.note == b.note && a.detail == b.detail);

    Index {
        root: root_display,
        present: true,
        legacy: None,
        pages: final_pages,
        tags: final_tags,
        documents: final_documents,
        gaps,
        findings,
    }
}

/// A page in progress: everything read off disk before links can be
/// resolved, which needs every other file's id known first.
struct PageDraft {
    id: String,
    title: String,
    frontmatter: bool,
    /// `true` only when frontmatter parsed *and* named a `title` -- the
    /// v1 findings about frontmatter content (`unsourced`,
    /// `incomplete_frontmatter`) are scoped to exactly this, per the issue.
    has_titled_frontmatter: bool,
    area: Option<String>,
    status: Option<String>,
    updated: Option<String>,
    source_paths: Vec<String>,
    tags: BTreeSet<String>,
    /// The text after the closing `---`, or the whole file when there is no
    /// frontmatter block. Read only to extract links, tags and document
    /// references; it is never kept once `index` returns.
    body: String,
}

struct DocumentDraft {
    id: String,
    ext: String,
    bytes: u64,
}

/// Read one `.md` file into a draft, pushing a `truncated` finding for one
/// that is too large to parse. Never fails outright: a read error, a missing
/// frontmatter block or unparseable YAML all fall back to the file name as
/// the title and the whole file as the body -- the file is still a page and
/// a graph node either way.
fn read_page(rel: &str, abs: &Path, findings: &mut Vec<Finding>) -> PageDraft {
    let id = rel[..rel.len() - 3].to_string();
    let file_name_title = id.rsplit('/').next().unwrap_or(&id).to_string();
    let fallback = |body: String| PageDraft {
        id: id.clone(),
        title: file_name_title.clone(),
        frontmatter: false,
        has_titled_frontmatter: false,
        area: None,
        status: None,
        updated: None,
        source_paths: Vec::new(),
        tags: extract_tags(&body),
        body,
    };

    let Ok(meta) = fs::metadata(abs) else {
        return fallback(String::new());
    };
    if meta.len() > MAX_PAGE_BYTES {
        findings.push(Finding {
            kind: FindingKind::Truncated,
            note: rel.to_string(),
            detail: "file exceeds the 1 MiB cap and was not read".into(),
        });
        return fallback(String::new());
    }
    let Ok(content) = fs::read_to_string(abs) else {
        return fallback(String::new());
    };

    let Some((yaml, body)) = split_frontmatter(&content) else {
        return fallback(content);
    };
    let Ok(fm) = serde_yaml_ng::from_str::<RawFrontMatter>(yaml) else {
        // The `---` delimiters were there, but what is between them is not
        // frontmatter this module can use -- still a page, and the text past
        // the second `---` is still its body.
        return fallback(body.to_string());
    };

    let mut tags = extract_tags(body);
    tags.extend(fm.tags.iter().flat_map(|f| f.values()));
    tags.extend(fm.keywords.iter().flat_map(|f| f.values()));
    let tags: BTreeSet<String> = tags.into_iter().map(|t| t.to_lowercase()).collect();

    let has_titled_frontmatter = fm.title.is_some();
    PageDraft {
        id,
        title: fm.title.unwrap_or(file_name_title),
        frontmatter: true,
        has_titled_frontmatter,
        area: fm.area,
        status: fm.status,
        updated: fm.updated.as_ref().and_then(scalar_to_string),
        source_paths: fm.sources.iter().map(|s| s.path().to_string()).collect(),
        tags,
        body: body.to_string(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Md,
    Other,
}

struct Walk {
    pages: Vec<(String, PathBuf)>,
    pages_truncated: bool,
    documents: Vec<(String, PathBuf)>,
    documents_truncated: bool,
}

/// Every regular file under `vault_root`, recursively, skipping dotfiles and
/// dot-directories and never following a symlink -- so `.obsidian/` can sit
/// in the vault untouched, and the vault can be opened in Obsidian directly.
/// A `.md` file (case-insensitive) is a page; every other file is a document.
/// Each bucket stops at its own cap and says so, rather than reading an
/// unbounded tree; the whole walk stops early only once both caps are hit,
/// since nothing more could be added to either list past that point.
fn discover(vault_root: &Path) -> Walk {
    let mut pages = Vec::new();
    let mut documents = Vec::new();
    let mut pages_truncated = false;
    let mut documents_truncated = false;
    let mut stack = vec![(vault_root.to_path_buf(), String::new())];
    while let Some((dir, rel_dir)) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else { continue };
        // Sorted so that which files land on the right side of a cap is
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
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_symlink() {
                continue;
            }
            let rel = if rel_dir.is_empty() { name_str.clone() } else { format!("{rel_dir}/{name_str}") };
            if file_type.is_dir() {
                stack.push((entry.path(), rel));
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            match classify_file(&name_str) {
                FileKind::Md => {
                    if pages.len() < MAX_PAGES {
                        pages.push((rel, entry.path()));
                    } else {
                        pages_truncated = true;
                    }
                }
                FileKind::Other => {
                    if documents.len() < MAX_DOCUMENTS {
                        documents.push((rel, entry.path()));
                    } else {
                        documents_truncated = true;
                    }
                }
            }
        }
        if pages_truncated && documents_truncated {
            break;
        }
    }
    Walk { pages, pages_truncated, documents, documents_truncated }
}

fn classify_file(name: &str) -> FileKind {
    if name.to_ascii_lowercase().ends_with(".md") {
        FileKind::Md
    } else {
        FileKind::Other
    }
}

// ============================================================== frontmatter

/// Frontmatter as YAML actually allows it to be written. No
/// `deny_unknown_fields`: an unrecognised key is tolerated, not refused.
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
    #[serde(default)]
    tags: Option<TagsField>,
    #[serde(default)]
    keywords: Option<TagsField>,
}

/// `tags:`/`keywords:` as the issue allows them: a single string, or a list.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum TagsField {
    One(String),
    Many(Vec<String>),
}

impl TagsField {
    fn values(&self) -> Vec<String> {
        match self {
            TagsField::One(s) => vec![s.clone()],
            TagsField::Many(v) => v.clone(),
        }
    }
}

/// `sources:` entries as they actually appear: a bare string (treated as the
/// path, with no access date), or a `{path, accessed}` map. `accessed` and
/// any other key a map carries are read and discarded -- this module counts
/// sources and checks their paths, and never needs to know when one was last
/// looked at.
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
        other => serde_yaml_ng::to_string(other).ok().map(|s| s.trim().to_string()),
    }
}

/// Split a file into its frontmatter YAML and its body. `None` when the file
/// does not start with a `---` line, or that line's block never closes --
/// either way, this module has no frontmatter to use, though the file is
/// still a page. A leading UTF-8 BOM (`\u{feff}`) is stripped first: some
/// editors write one, and it would otherwise sit in front of the `---` and
/// make every such file look frontmatter-less.
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

// ==================================================================== tags

/// Every inline `#tag` and frontmatter tag this page carries, lowercased.
/// Obsidian's rules: `#` followed by letters, digits, `_`, `-` or `/`
/// (nested tags like `#area/sub` kept whole); a purely numeric `#123` is not
/// a tag; nothing inside inline code or a fenced block counts; a heading
/// line does not count; and a `#` inside a URL does not count -- caught not
/// by parsing URLs but by requiring the character *before* the `#` to be
/// whitespace or the start of the line, which a `#` glued to the end of a
/// word (`.../y#frag`) never is.
fn extract_tags(body: &str) -> BTreeSet<String> {
    let mut tags = BTreeSet::new();
    let mut fenced_with: Option<&str> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if let Some(marker) = fenced_with {
            if trimmed.starts_with(marker) {
                fenced_with = None;
            }
            continue; // nothing inside (or the closing line of) a fence counts
        }
        if let Some(marker) = fence_open(trimmed) {
            fenced_with = Some(marker);
            continue;
        }
        if is_heading_line(trimmed) {
            continue;
        }
        scan_line_for_tags(line, &mut tags);
    }
    tags
}

fn fence_open(trimmed: &str) -> Option<&'static str> {
    if trimmed.starts_with("```") {
        Some("```")
    } else if trimmed.starts_with("~~~") {
        Some("~~~")
    } else {
        None
    }
}

/// An ATX heading: one to six `#` at the start of the (already left-trimmed)
/// line, then whitespace or the end of the line.
fn is_heading_line(trimmed: &str) -> bool {
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return false;
    }
    match trimmed[hashes..].chars().next() {
        None => true,
        Some(c) => c.is_whitespace(),
    }
}

fn is_tag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '/'
}

fn scan_line_for_tags(line: &str, tags: &mut BTreeSet<String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut in_code = false;
    let mut i = 0usize;
    let mut prev: Option<char> = None;
    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            in_code = !in_code;
            prev = Some(c);
            i += 1;
            continue;
        }
        if !in_code && c == '#' && prev.is_none_or(|p| p.is_whitespace()) {
            let start = i + 1;
            let mut j = start;
            while j < chars.len() && is_tag_char(chars[j]) {
                j += 1;
            }
            if j > start {
                let raw: String = chars[start..j].iter().collect();
                if !raw.chars().all(|c| c.is_ascii_digit()) {
                    tags.insert(raw.to_lowercase());
                }
                prev = chars.get(j.saturating_sub(1)).copied();
                i = j;
                continue;
            }
        }
        prev = Some(c);
        i += 1;
    }
}

// =============================================================== extraction

/// A link target longer than this is not a link -- it is prose that happened
/// to sit between two delimiters (or never closed until some distant,
/// unrelated one). No real page id, tag or file name comes close to this;
/// the cap exists purely so an unbounded span of body text can never reach
/// the payload.
const MAX_LINK_TARGET_LEN: usize = 200;

/// Whether `target` (already stripped of a `|label`/`#heading` suffix and
/// trimmed) is shaped like something a person actually wrote as a link, as
/// opposed to text that merely landed between two delimiters -- a
/// 5,000-character paragraph, or a bash `[[ -f x ]]` test caught by the same
/// bracket pair. A real target may contain a plain space, but never a
/// bracket, a tab or a line break.
fn is_link_target_shape(target: &str) -> bool {
    if target.is_empty() || target.ends_with('/') {
        return false;
    }
    if target.chars().count() > MAX_LINK_TARGET_LEN {
        return false;
    }
    !target.chars().any(|c| c == '[' || c == ']' || c == '(' || c == ')' || (c.is_whitespace() && c != ' '))
}

/// Every `[[...]]` in `body`, stripped of a `|label` or `#heading` suffix and
/// trimmed. `![[...]]` needs no separate handling: the `!` sits outside the
/// `[[`, so it is simply the character before a match this already finds. A
/// span that is not shaped like a real link target is skipped entirely,
/// rather than resolved or listed as a gap, and scanning always resumes from
/// just past the `]]` that closed the span under consideration, so a
/// rejected span never swallows a valid one later on the same line.
fn extract_wiki_links(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel_start) = body[cursor..].find("[[") {
        let start = cursor + rel_start + 2;
        let Some(rel_end) = body[start..].find("]]") else { break };
        let end = start + rel_end;
        let raw = &body[start..end];
        cursor = end + 2;
        if raw.contains(['\n', '\r']) || raw.starts_with(char::is_whitespace) || raw.ends_with(char::is_whitespace) {
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

/// Every Markdown `[text](target)` in `body`, `target` extracted and
/// cleaned: an optional `"Title"` after a space is dropped, a `#fragment` is
/// dropped, and an external (`scheme://`) or directory (`.../`) target is
/// skipped entirely -- neither is ever a page or a document in this vault.
fn extract_markdown_links(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel_start) = body[cursor..].find('[') {
        let bracket_start = cursor + rel_start;
        let Some(close_rel) = body[bracket_start + 1..].find(']') else { break };
        let bracket_end = bracket_start + 1 + close_rel;
        cursor = bracket_end + 1;
        if !body[cursor..].starts_with('(') {
            continue;
        }
        let paren_start = cursor + 1;
        let Some(paren_end_rel) = body[paren_start..].find(')') else { continue };
        let paren_end = paren_start + paren_end_rel;
        cursor = paren_end + 1;
        if let Some(target) = parse_markdown_target(body[paren_start..paren_end].trim()) {
            out.push(target);
        }
    }
    out
}

fn parse_markdown_target(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    // `path "Title"` or `path 'Title'` -- keep only the path.
    let path_part = match raw.find(char::is_whitespace) {
        Some(i) => &raw[..i],
        None => raw,
    };
    if path_part.contains("://") || path_part.starts_with('#') {
        return None;
    }
    let target = path_part.split('#').next().unwrap_or(path_part).trim();
    if !is_link_target_shape(target) {
        return None;
    }
    Some(target.to_string())
}

/// Whichever kind of node a link target names, decided by its extension: a
/// bare name or one ending `.md` is a page (the `.md` stripped before
/// resolving, since page ids never carry it); anything else names a
/// document, extension and all.
enum LinkClass {
    Page(String),
    Document(String),
}

fn classify_link(target: &str) -> LinkClass {
    match split_extension(target) {
        Some((base, ext)) if ext.eq_ignore_ascii_case("md") => LinkClass::Page(base.to_string()),
        Some(_) => LinkClass::Document(target.to_string()),
        None => LinkClass::Page(target.to_string()),
    }
}

/// `target`'s extension, read only from its last path segment so a `.` in an
/// earlier directory component is never mistaken for one.
fn split_extension(target: &str) -> Option<(&str, &str)> {
    let seg_start = target.rfind('/').map(|i| i + 1).unwrap_or(0);
    let last = &target[seg_start..];
    let dot = last.rfind('.')?;
    if dot == 0 {
        return None; // a dotfile-shaped name, not an extension
    }
    Some((&target[..seg_start + dot], &last[dot + 1..]))
}

fn extension_of(path: &str) -> Option<&str> {
    split_extension(path).map(|(_, ext)| ext)
}

// ============================================================== resolution

enum Resolution {
    Found(String),
    Ambiguous(Vec<String>),
    Unresolved,
}

/// The wiki-root-relative directory a page lives in -- `""` for a page at
/// the vault root, the parent path otherwise.
fn dir_of(id: &str) -> &str {
    match id.rfind('/') {
        Some(i) => &id[..i],
        None => "",
    }
}

/// Join `base_dir` (a vault-root-relative directory, `/`-separated) and
/// `target` (a link target, also `/`-separated) and resolve `.`/`..`
/// components without touching the filesystem. `None` when the result would
/// climb above the vault root, or when it normalises to nothing at all --
/// either way, this resolution rule simply does not match, and the caller
/// moves on to the next one.
fn normalize_relative(base_dir: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = if base_dir.is_empty() { Vec::new() } else { base_dir.split('/').collect() };
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
/// target-directory-relative, then root-relative, then a unique file stem.
/// Shared by pages and documents alike -- `known_ids` and `by_stem` are
/// whichever namespace the caller means.
fn resolve(target: &str, dir: &str, known_ids: &HashSet<String>, by_stem: &HashMap<String, Vec<String>>) -> Resolution {
    if let Some(id) = normalize_relative(dir, target) {
        if known_ids.contains(&id) {
            return Resolution::Found(id);
        }
    }
    if let Some(id) = normalize_relative("", target) {
        if known_ids.contains(&id) {
            return Resolution::Found(id);
        }
    }
    let last = target.rsplit('/').next().unwrap_or(target);
    if let Some(candidates) = by_stem.get(last) {
        return match candidates.len() {
            1 => Resolution::Found(candidates[0].clone()),
            _ => Resolution::Ambiguous(candidates.clone()),
        };
    }
    Resolution::Unresolved
}

// ================================================================== sources

/// Bound how much of a user-controlled string (a link target that already
/// passed `is_link_target_shape`, but also a `sources[].path` that has no
/// such cap at all) a finding's `detail` may embed.
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
/// module ever calls `fs::metadata` on the path at all.
fn is_secret_source_field(path: &str) -> bool {
    let comps: Vec<&str> = path.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").collect();
    comps.len() >= 2 && comps[0].eq_ignore_ascii_case("data") && comps[1].eq_ignore_ascii_case("secrets")
}

/// Whether `path` is the kind of `sources[].path` `missing_source` may stat:
/// relative, with no `..` component, and not a URL.
fn eligible_for_stat(path: &str) -> bool {
    if path.contains("://") {
        return false;
    }
    let p = Path::new(path);
    if p.is_absolute() {
        return false;
    }
    !p.components().any(|c| matches!(c, Component::ParentDir))
}

// =========================================================== writing (import/add)

/// A file over this size is refused rather than copied. 50 MiB.
pub const MAX_WRITE_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// An import past this many files ends early, `truncated: true`, with
/// whatever was already copied kept.
pub const MAX_IMPORT_FILES: usize = 10_000;

/// One refusal: `path` is the target this write was about to create, or
/// `None` when naming it would repeat the very thing being refused -- a
/// source under `data/secrets/` or elsewhere under `.factory/` outside the
/// vault. `reason` never embeds a path either, in that case.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Refusal {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub reason: String,
}

/// What `import` or `add` did. `copied`/`skipped_existing`/`skipped_hidden`
/// name vault-relative targets; `truncated` is set once `import` stops early
/// at `MAX_IMPORT_FILES`, with everything found before that point kept.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WriteResult {
    pub copied: Vec<String>,
    pub skipped_existing: Vec<String>,
    pub skipped_hidden: Vec<String>,
    pub refused: Vec<Refusal>,
    #[serde(default)]
    pub truncated: bool,
}

/// Whether `target` (a vault-relative path some write is about to create) is
/// refused outright, decided from the string alone: empty, absolute, or
/// climbing out with a `..` component all fail to name anywhere inside the
/// vault. A symlinked parent directory is a filesystem fact rather than a
/// string one, and is checked only once a write is actually attempted.
pub fn refuse_target(target: &str) -> Option<String> {
    if target.trim().is_empty() {
        return Some("no target path given".into());
    }
    let p = Path::new(target);
    if p.is_absolute() {
        return Some("target escapes the vault: an absolute path".into());
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Some("target escapes the vault: a `..` component".into());
    }
    None
}

/// Whether `source` (an absolute path this write wants to copy *from*) is
/// refused outright: it lies under `<root>/data/secrets/`, or elsewhere
/// under `<root>/.factory/` besides the vault itself. Decided from the path
/// string alone -- no `fs::metadata`, no `fs::canonicalize` -- so refusing a
/// source never so much as confirms the source exists, and the caller's
/// reason string never repeats the path back.
pub fn refuse_source(root: &Path, source: &Path) -> Option<&'static str> {
    let rel = lexical_relative(root, source)?;
    let comps: Vec<&str> = rel.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    if comps.first().is_some_and(|c| c.eq_ignore_ascii_case("data")) && comps.get(1).is_some_and(|c| c.eq_ignore_ascii_case("secrets")) {
        return Some("a source under data/secrets/ is never read");
    }
    if comps.first().is_some_and(|c| c.eq_ignore_ascii_case(".factory")) && !comps.get(1).is_some_and(|c| c.eq_ignore_ascii_case("knowledge")) {
        return Some("a source under .factory/ outside the vault is never read");
    }
    None
}

/// `candidate`'s path relative to `root`, computed purely by comparing path
/// components after resolving `.`/`..` lexically -- never by asking the
/// filesystem, which `fs::canonicalize` would (and would also, unlike this,
/// follow a symlink). `None` when `candidate` does not lie under `root` at
/// all, which is the ordinary case: most import sources live nowhere near
/// the instance root.
fn lexical_relative(root: &Path, candidate: &Path) -> Option<String> {
    let root = normalize_lexical(root);
    let candidate = normalize_lexical(candidate);
    let mut r = root.components();
    let mut c = candidate.components();
    for rc in r.by_ref() {
        match c.next() {
            Some(cc) if cc == rc => continue,
            _ => return None,
        }
    }
    let rest: Vec<String> = c.map(|comp| comp.as_os_str().to_string_lossy().into_owned()).collect();
    if rest.is_empty() {
        None
    } else {
        Some(rest.join("/"))
    }
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

enum WriteOutcome {
    Copied,
    SkippedExisting,
}

/// The one place that actually touches the vault: refuse a target that
/// escapes it or sits behind a symlinked parent, refuse a file over the size
/// cap, skip one that already exists unless `overwrite`, then copy. Every
/// check above the copy itself is a filesystem fact -- the string-only
/// checks already ran in the caller, before this was ever reached.
fn copy_into_vault(dest_base: &Path, rel: &str, source: &Path, overwrite: bool) -> Result<WriteOutcome, String> {
    if let Some(reason) = refuse_target(rel) {
        return Err(reason);
    }
    let target = dest_base.join(rel);
    if let Some(parent) = target.parent() {
        if fs::symlink_metadata(parent).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            return Err("target escapes the vault: a parent directory is a symlink".into());
        }
    }
    let meta = fs::metadata(source).map_err(|e| format!("could not read the source: {e}"))?;
    if meta.len() > MAX_WRITE_FILE_BYTES {
        return Err(format!("file exceeds the {}MiB cap", MAX_WRITE_FILE_BYTES / (1024 * 1024)));
    }
    if target.exists() && !overwrite {
        return Ok(WriteOutcome::SkippedExisting);
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    fs::copy(source, &target).map_err(|e| format!("copy failed: {e}"))?;
    Ok(WriteOutcome::Copied)
}

/// `factory knowledge import <source_dir> [--into] [--overwrite]`: copy every
/// file under `source_dir` into the vault, preserving relative paths so a
/// page's `[[area/page]]` links still resolve afterwards. Pages and
/// documents travel together; nothing already in the vault is ever edited,
/// and nothing is ever deleted. Runs in `spawn_blocking` -- this walks and
/// copies for real.
pub fn import(root: &Path, source_dir: &Path, into: Option<&str>, overwrite: bool) -> Result<WriteResult, String> {
    if let Some(reason) = refuse_source(root, source_dir) {
        return Err(reason.to_string());
    }
    let into_rel = into.unwrap_or("");
    if let Some(reason) = refuse_target_or_root(into_rel) {
        return Err(reason);
    }
    let dest_base = vault_root(root).join(into_rel);

    let mut result = WriteResult::default();
    let mut count = 0usize;
    let mut stack = vec![String::new()];
    while let Some(rel_dir) = stack.pop() {
        let dir = source_dir.join(&rel_dir);
        let Ok(read) = fs::read_dir(&dir) else { continue };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name_str = entry.file_name().to_string_lossy().to_string();
            let rel = if rel_dir.is_empty() { name_str.clone() } else { format!("{rel_dir}/{name_str}") };
            if name_str.starts_with('.') {
                result.skipped_hidden.push(rel);
                continue;
            }
            let Ok(file_type) = entry.file_type() else { continue };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                stack.push(rel);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if count >= MAX_IMPORT_FILES {
                result.truncated = true;
                return Ok(result);
            }
            count += 1;
            match copy_into_vault(&dest_base, &rel, &entry.path(), overwrite) {
                Ok(WriteOutcome::Copied) => result.copied.push(join_into(into_rel, &rel)),
                Ok(WriteOutcome::SkippedExisting) => result.skipped_existing.push(join_into(into_rel, &rel)),
                Err(reason) => result.refused.push(Refusal { path: Some(join_into(into_rel, &rel)), reason }),
            }
        }
    }
    Ok(result)
}

/// `factory knowledge add <file>... [--into] [--overwrite]`: add one or more
/// files by path. Without `--into`, each file finds its own default -- the
/// vault root for a `.md` file, `documents/` for anything else -- the way
/// Obsidian's own attachments folder works; `--into` overrides that default
/// for every file in the call alike.
pub fn add(root: &Path, sources: &[PathBuf], into: Option<&str>, overwrite: bool) -> WriteResult {
    let mut result = WriteResult::default();
    for source in sources {
        let Some(name) = source.file_name().map(|n| n.to_string_lossy().to_string()) else {
            result.refused.push(Refusal { path: None, reason: "not a file name".into() });
            continue;
        };
        if name.starts_with('.') {
            result.skipped_hidden.push(name);
            continue;
        }
        if let Some(reason) = refuse_source(root, source) {
            result.refused.push(Refusal { path: None, reason: reason.to_string() });
            continue;
        }
        let default_into = if name.to_ascii_lowercase().ends_with(".md") { "" } else { "documents" };
        let into_rel = into.unwrap_or(default_into);
        if let Some(reason) = refuse_target_or_root(into_rel) {
            result.refused.push(Refusal { path: Some(name.clone()), reason });
            continue;
        }
        let dest_base = vault_root(root).join(into_rel);
        match copy_into_vault(&dest_base, &name, source, overwrite) {
            Ok(WriteOutcome::Copied) => result.copied.push(join_into(into_rel, &name)),
            Ok(WriteOutcome::SkippedExisting) => result.skipped_existing.push(join_into(into_rel, &name)),
            Err(reason) => result.refused.push(Refusal { path: Some(join_into(into_rel, &name)), reason }),
        }
    }
    result
}

/// `PUT /api/knowledge/files?path=&overwrite=`: write one file's bytes
/// directly, the browser upload's only path in -- it cannot name a location
/// on the daemon's own disk, so it sends the content instead of a source
/// path. `path` is the full vault-relative target, exactly as `import`/`add`
/// would compute one, and gets the same refusals.
pub fn write_bytes(root: &Path, path: &str, overwrite: bool, bytes: &[u8]) -> Result<String, String> {
    if let Some(reason) = refuse_target(path) {
        return Err(reason);
    }
    let name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if name.starts_with('.') {
        return Err("refused: a dotfile is never written".into());
    }
    if bytes.len() as u64 > MAX_WRITE_FILE_BYTES {
        return Err(format!("file exceeds the {}MiB cap", MAX_WRITE_FILE_BYTES / (1024 * 1024)));
    }
    let target = vault_root(root).join(path);
    if let Some(parent) = target.parent() {
        if fs::symlink_metadata(parent).map(|m| m.file_type().is_symlink()).unwrap_or(false) {
            return Err("target escapes the vault: a parent directory is a symlink".into());
        }
    }
    if target.exists() && !overwrite {
        return Err("already exists; pass overwrite to replace it".into());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    fs::write(&target, bytes).map_err(|e| e.to_string())?;
    Ok(path.to_string())
}

/// `refuse_target`, but treating the vault root itself (`""`) as always
/// allowed -- `--into` is optional, and an absent one means "the vault
/// root", not "no target given".
fn refuse_target_or_root(into_rel: &str) -> Option<String> {
    if into_rel.is_empty() {
        return None;
    }
    refuse_target(into_rel)
}

fn join_into(into_rel: &str, rel: &str) -> String {
    if into_rel.is_empty() {
        rel.to_string()
    } else {
        format!("{into_rel}/{rel}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("factory-knowledge-test-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(vault_root(&dir)).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let path = vault_root(root).join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn page_by_id<'a>(idx: &'a Index, id: &str) -> &'a Page {
        idx.pages.iter().find(|p| p.id == id).unwrap_or_else(|| panic!("no page {id}"))
    }

    // -- pages: every .md is a node, title falls back, frontmatter is optional

    #[test]
    fn a_page_without_frontmatter_is_still_a_graph_node_titled_by_its_file_name() {
        let root = temp_root("no-frontmatter");
        write(&root, "index.md", "# Index\n\nSee [[some-page]].\n");
        write(&root, "some-page.md", "Nothing fancy here.\n");

        let idx = index(&root);
        assert_eq!(idx.pages.len(), 2);
        let index_page = page_by_id(&idx, "index");
        assert_eq!(index_page.title, "index");
        assert!(!index_page.frontmatter);
        assert!(index_page.links.contains(&"some-page".to_string()), "{:?}", index_page.links);
        let target = page_by_id(&idx, "some-page");
        assert!(target.backlinks.contains(&"index".to_string()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn frontmatter_without_a_title_still_falls_back_to_the_file_name() {
        let root = temp_root("frontmatter-no-title");
        write(&root, "area/log.md", "---\narea: ops\n---\nAppend-only.\n");

        let idx = index(&root);
        let page = page_by_id(&idx, "area/log");
        assert_eq!(page.title, "log");
        assert!(page.frontmatter);
        assert_eq!(page.area.as_deref(), Some("ops"));
        // unsourced/incomplete_frontmatter are scoped to pages whose
        // frontmatter names a title -- this one's does not, so neither fires
        // (it is still an orphan, being the only page in the vault, which is
        // a separate finding this test is not about).
        assert!(
            idx.findings
                .iter()
                .all(|f| f.note != "area/log" || matches!(f.kind, FindingKind::Orphan)),
            "{:?}",
            idx.findings
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn unsourced_and_incomplete_frontmatter_only_fire_for_a_titled_page() {
        let root = temp_root("titled-findings");
        write(&root, "titled.md", "---\ntitle: Titled\n---\nNo links.\n");
        write(&root, "untitled.md", "---\narea: x\n---\nNo links.\n");

        let idx = index(&root);
        assert!(idx.findings.iter().any(|f| f.kind == FindingKind::Unsourced && f.note == "titled"));
        assert!(idx.findings.iter().any(|f| f.kind == FindingKind::IncompleteFrontmatter && f.note == "titled"));
        assert!(
            !idx.findings.iter().any(|f| f.note == "untitled"
                && matches!(f.kind, FindingKind::Unsourced | FindingKind::IncompleteFrontmatter)),
            "{:?}",
            idx.findings
        );

        std::fs::remove_dir_all(&root).ok();
    }

    // -- link resolution, carried over from v1 plus the new document/markdown forms

    #[test]
    fn page_relative_root_relative_and_bare_name_resolution_all_work() {
        let root = temp_root("resolution");
        write(&root, "partners/acme.md", "---\ntitle: Acme Co\n---\nSee [[jane-doe]] and [[roadmap]].\n");
        write(&root, "partners/jane-doe.md", "---\ntitle: Jane Doe\n---\nContact for [[partners/acme]].\n");
        write(&root, "gadgets/roadmap.md", "---\ntitle: Gadget Roadmap\n---\nNothing links out of here.\n");

        let idx = index(&root);
        let acme = page_by_id(&idx, "partners/acme");
        assert!(acme.links.contains(&"partners/jane-doe".to_string()));
        assert!(acme.links.contains(&"gadgets/roadmap".to_string()));
        let jane = page_by_id(&idx, "partners/jane-doe");
        assert!(jane.links.contains(&"partners/acme".to_string()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_markdown_link_to_a_page_is_a_page_edge() {
        let root = temp_root("markdown-page-link");
        write(&root, "company/pricing.md", "---\ntitle: Pricing\n---\nSee [Acme](../partners/acme.md).\n");
        write(&root, "partners/acme.md", "---\ntitle: Acme\n---\nNothing here.\n");

        let idx = index(&root);
        let pricing = page_by_id(&idx, "company/pricing");
        assert_eq!(pricing.links, vec!["partners/acme".to_string()]);
        assert!(pricing.gaps.is_empty());
        let acme = page_by_id(&idx, "partners/acme");
        assert!(acme.backlinks.contains(&"company/pricing".to_string()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_external_markdown_link_is_never_a_gap() {
        let root = temp_root("external-link");
        write(&root, "a.md", "---\ntitle: A\n---\nSee [the wiki](https://example.com/x#y).\n");

        let idx = index(&root);
        assert!(idx.gaps.is_empty(), "{:?}", idx.gaps);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn document_references_resolve_through_all_three_link_forms() {
        let root = temp_root("document-refs");
        write(&root, "documents/acme-contract.pdf", "not real pdf bytes");
        write(
            &root,
            "partners/acme.md",
            "---\ntitle: Acme\n---\nSee ![[acme-contract.pdf]], also [[acme-contract.pdf]], and [the contract](../documents/acme-contract.pdf).\n",
        );

        let idx = index(&root);
        let acme = page_by_id(&idx, "partners/acme");
        assert_eq!(acme.documents, vec!["documents/acme-contract.pdf".to_string()]);
        let doc = idx.documents.iter().find(|d| d.id == "documents/acme-contract.pdf").expect("document indexed");
        assert_eq!(doc.ext, "pdf");
        assert!(doc.referenced_by.contains(&"partners/acme".to_string()));
        assert!(acme.gaps.is_empty(), "{:?}", acme.gaps);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_ambiguous_document_name_is_reported_and_left_unresolved() {
        let root = temp_root("ambiguous-document");
        write(&root, "east/contract.pdf", "east bytes");
        write(&root, "west/contract.pdf", "west bytes");
        write(&root, "general/other.md", "---\ntitle: Other\n---\nSee ![[contract.pdf]].\n");

        let idx = index(&root);
        let other = page_by_id(&idx, "general/other");
        assert!(other.documents.is_empty(), "{:?}", other.documents);
        assert!(other.gaps.contains(&"contract.pdf".to_string()));
        assert!(
            idx.findings
                .iter()
                .any(|f| f.kind == FindingKind::AmbiguousLink
                    && f.note == "general/other"
                    && f.detail.contains("matches more than one document")),
            "{:?}",
            idx.findings
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unresolved_document_reference_is_a_gap() {
        let root = temp_root("document-gap");
        write(&root, "a.md", "---\ntitle: A\n---\nSee ![[missing.pdf]].\n");

        let idx = index(&root);
        assert!(idx.gaps.iter().any(|g| g.target == "missing.pdf" && g.from == vec!["a".to_string()]));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_ambiguous_file_name_is_reported_and_left_unresolved() {
        let root = temp_root("ambiguous");
        write(&root, "clients/contact.md", "---\ntitle: Clients Contact\n---\nNo links.\n");
        write(&root, "ops/contact.md", "---\ntitle: Ops Contact\n---\nNo links.\n");
        write(&root, "general/other.md", "---\ntitle: General Notes\n---\nTalk to [[contact]] about it.\n");

        let idx = index(&root);
        let other = page_by_id(&idx, "general/other");
        assert!(other.links.is_empty());
        assert!(other.gaps.contains(&"contact".to_string()));
        assert!(idx.findings.iter().any(|f| f.kind == FindingKind::AmbiguousLink && f.note == "general/other"));

        std::fs::remove_dir_all(&root).ok();
    }

    // -- tags

    #[test]
    fn frontmatter_tags_and_keywords_accept_a_string_or_a_list() {
        let root = temp_root("frontmatter-tags");
        write(&root, "a.md", "---\ntitle: A\ntags: solo\nkeywords: [alpha, Beta]\n---\nBody.\n");

        let idx = index(&root);
        let a = page_by_id(&idx, "a");
        assert_eq!(a.tags, vec!["alpha".to_string(), "beta".to_string(), "solo".to_string()]);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn inline_tags_follow_obsidians_rules() {
        let root = temp_root("inline-tags");
        write(
            &root,
            "a.md",
            "# A heading with #not-a-tag\n\nReal tags: #Pricing and #area/sub. Not numeric: #123. \
             Not in code: `#also-not-a-tag`. Not a URL: https://example.com/x#frag.\n\n```\n#fenced-not-a-tag\n```\n",
        );

        let idx = index(&root);
        let a = page_by_id(&idx, "a");
        assert_eq!(a.tags, vec!["area/sub".to_string(), "pricing".to_string()], "{:?}", a.tags);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tags_produce_tag_nodes_listing_every_page_that_carries_them() {
        let root = temp_root("tag-nodes");
        write(&root, "a.md", "---\ntitle: A\ntags: pricing\n---\nNo links.\n");
        write(&root, "b.md", "---\ntitle: B\n---\nSee #pricing here.\n");

        let idx = index(&root);
        let tag = idx.tags.iter().find(|t| t.name == "pricing").expect("pricing tag");
        assert_eq!(tag.pages, vec!["a".to_string(), "b".to_string()]);

        std::fs::remove_dir_all(&root).ok();
    }

    // -- orphan, redefined

    #[test]
    fn a_page_carrying_only_a_tag_is_not_an_orphan() {
        let root = temp_root("orphan-tag");
        write(&root, "a.md", "---\ntitle: A\ntags: pricing\n---\nNo links, no backlinks.\n");

        let idx = index(&root);
        assert!(!idx.findings.iter().any(|f| f.kind == FindingKind::Orphan && f.note == "a"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_page_referencing_only_a_document_is_not_an_orphan() {
        let root = temp_root("orphan-doc");
        write(&root, "documents/x.pdf", "bytes");
        write(&root, "a.md", "---\ntitle: A\n---\nSee ![[x.pdf]].\n");

        let idx = index(&root);
        assert!(!idx.findings.iter().any(|f| f.kind == FindingKind::Orphan && f.note == "a"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_page_with_no_backlinks_tags_or_documents_is_an_orphan_even_if_it_links_out() {
        let root = temp_root("orphan-plain");
        write(&root, "a.md", "---\ntitle: A\n---\nSee [[b]].\n");
        write(&root, "b.md", "---\ntitle: B\n---\nNo links.\n");

        let idx = index(&root);
        assert!(idx.findings.iter().any(|f| f.kind == FindingKind::Orphan && f.note == "a"), "{:?}", idx.findings);
        assert!(!idx.findings.iter().any(|f| f.kind == FindingKind::Orphan && f.note == "b"));

        std::fs::remove_dir_all(&root).ok();
    }

    // -- vault presence, legacy path, walk limits

    #[test]
    fn a_missing_vault_is_an_empty_state_naming_the_legacy_path_when_it_exists() {
        let root = std::env::temp_dir().join(format!("factory-knowledge-test-missing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("knowledge/wiki")).unwrap();

        let idx = index(&root);
        assert!(!idx.present);
        assert!(idx.pages.is_empty());
        assert!(idx.root.ends_with(".factory/knowledge") || idx.root.ends_with(".factory\\knowledge"));
        assert_eq!(idx.legacy.as_deref(), Some(root.join("knowledge/wiki").display().to_string()).as_deref());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn legacy_is_never_set_once_the_vault_is_present() {
        let root = temp_root("legacy-not-checked");
        std::fs::create_dir_all(root.join("knowledge/wiki")).unwrap();
        write(&root, "a.md", "hello\n");

        let idx = index(&root);
        assert!(idx.present);
        assert!(idx.legacy.is_none());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn every_document_is_stat_ed_never_opened() {
        let root = temp_root("documents-stat-only");
        let path = vault_root(&root).join("documents/report.csv");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "a,b,c\n1,2,3\n").unwrap();

        let idx = index(&root);
        let doc = idx.documents.iter().find(|d| d.id == "documents/report.csv").expect("document indexed");
        assert_eq!(doc.ext, "csv");
        assert_eq!(doc.bytes, "a,b,c\n1,2,3\n".len() as u64);
        let json = serde_json::to_string(&idx).unwrap();
        assert!(!json.contains("a,b,c"), "document bytes must never appear in the payload");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn dotfiles_and_dot_directories_are_skipped_so_obsidian_config_can_sit_in_the_vault() {
        let root = temp_root("dotfiles");
        write(&root, ".obsidian/workspace.json", "{}");
        write(&root, ".hidden-page.md", "hidden");
        write(&root, "visible.md", "visible");

        let idx = index(&root);
        assert_eq!(idx.pages.len(), 1);
        assert!(idx.documents.is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn hitting_the_page_cap_is_a_truncated_finding_never_an_error() {
        let root = temp_root("page-cap");
        for i in 0..(MAX_PAGES + 5) {
            write(&root, &format!("p{i:05}.md", ), "no frontmatter\n");
        }

        let idx = index(&root);
        assert_eq!(idx.pages.len(), MAX_PAGES);
        assert!(idx.findings.iter().any(|f| f.kind == FindingKind::Truncated && f.detail.contains("pages")));

        std::fs::remove_dir_all(&root).ok();
    }

    // -- secret sources, byte-identical, no leaks

    #[test]
    fn a_source_under_data_secrets_is_flagged_without_ever_being_read() {
        let root = temp_root("secrets-proof");
        let secrets_dir = root.join("data/secrets");
        std::fs::create_dir_all(&secrets_dir).unwrap();
        std::fs::write(secrets_dir.join("x.md"), "sentinel-secret-body-should-never-leak").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&secrets_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        }

        write(&root, "citer.md", "---\ntitle: Citer\nsources:\n  - path: data/secrets/x.md\n---\nNo links.\n");
        let idx = index(&root);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&secrets_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        assert!(idx.findings.iter().any(|f| f.kind == FindingKind::SecretSource && f.note == "citer"));
        assert!(!idx.findings.iter().any(|f| f.kind == FindingKind::MissingSource && f.note == "citer"));
        let json = serde_json::to_string(&idx).unwrap();
        assert!(!json.contains("secrets/x"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn no_page_body_text_ever_reaches_the_payload() {
        let root = temp_root("no-body-leak");
        write(
            &root,
            "quiet.md",
            "---\ntitle: Quiet\n---\nSee [[SENTINEL-BODY-TEXT-f8a2c1\nshould never be serialized]].\n",
        );
        let idx = index(&root);
        let json = serde_json::to_string(&idx).unwrap();
        assert!(!json.contains("SENTINEL-BODY-TEXT-f8a2c1"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_same_files_always_produce_the_same_bytes() {
        let root = temp_root("determinism");
        write(&root, "a.md", "---\ntitle: A\ntags: pricing\n---\nSee [[b]] and [[missing]].\n");
        write(&root, "b.md", "---\ntitle: B\n---\nSee [[a]].\n");

        let first = serde_json::to_string(&index(&root)).unwrap();
        let second = serde_json::to_string(&index(&root)).unwrap();
        assert_eq!(first, second);

        std::fs::remove_dir_all(&root).ok();
    }

    // -- writing: import/add refusals

    #[test]
    fn a_target_that_escapes_the_vault_is_refused_from_the_string_alone() {
        assert!(refuse_target("../outside.md").is_some());
        assert!(refuse_target("/absolute.md").is_some());
        assert!(refuse_target("fine/relative.md").is_none());
    }

    #[test]
    fn a_source_under_data_secrets_is_refused_without_the_path_echoed() {
        let root = PathBuf::from("/instance");
        let reason = refuse_source(&root, Path::new("/instance/data/secrets/x.md")).expect("refused");
        assert!(!reason.contains("secrets/x"), "{reason}");
    }

    #[test]
    fn a_source_under_dot_factory_outside_the_vault_is_refused() {
        let root = PathBuf::from("/instance");
        assert!(refuse_source(&root, Path::new("/instance/.factory/factory.sqlite")).is_some());
        assert!(refuse_source(&root, Path::new("/instance/.factory/worktrees/w1")).is_some());
        // The vault itself is fine to import from -- re-importing a subtree,
        // or the legacy wiki once it has been copied in already.
        assert!(refuse_source(&root, Path::new("/instance/.factory/knowledge/area")).is_none());
    }

    #[test]
    fn a_source_outside_the_root_entirely_is_never_refused_by_these_two_rules() {
        let root = PathBuf::from("/instance");
        assert!(refuse_source(&root, Path::new("/elsewhere/my-vault")).is_none());
    }

    #[test]
    fn import_copies_a_tree_preserving_relative_paths_and_reports_what_it_did() {
        let root = temp_root("import-copy");
        let source = std::env::temp_dir().join(format!("factory-import-source-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(source.join("area")).unwrap();
        std::fs::write(source.join("area/page.md"), "---\ntitle: Page\n---\nBody.\n").unwrap();
        std::fs::write(source.join("area/doc.pdf"), "pdf bytes").unwrap();
        std::fs::write(source.join(".hidden"), "skip me").unwrap();

        let result = import(&root, &source, None, false).unwrap();
        assert!(result.copied.contains(&"area/page.md".to_string()), "{:?}", result.copied);
        assert!(result.copied.contains(&"area/doc.pdf".to_string()), "{:?}", result.copied);
        assert!(result.skipped_hidden.contains(&".hidden".to_string()));
        assert!(vault_root(&root).join("area/page.md").exists());

        let idx = index(&root);
        assert!(idx.pages.iter().any(|p| p.id == "area/page"), "the imported page resolves through the same walk");

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&source).ok();
    }

    #[test]
    fn import_skips_a_dotfile_nested_below_the_top_level_too() {
        let root = temp_root("import-nested-dotfile");
        let source = std::env::temp_dir().join(format!("factory-import-source-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(source.join("partners")).unwrap();
        std::fs::write(source.join("partners/acme.md"), "---\ntitle: Acme\n---\nBody.\n").unwrap();
        std::fs::write(source.join("partners/.DS_Store"), "skip me too").unwrap();

        let result = import(&root, &source, None, false).unwrap();
        assert!(result.copied.contains(&"partners/acme.md".to_string()), "{:?}", result.copied);
        assert!(
            result.skipped_hidden.contains(&"partners/.DS_Store".to_string()),
            "a dotfile below the top level must be skipped too, with its full relative path reported: {:?}",
            result.skipped_hidden
        );
        assert!(!vault_root(&root).join("partners/.DS_Store").exists());

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&source).ok();
    }

    #[test]
    fn import_skips_an_existing_target_unless_overwrite_is_set() {
        let root = temp_root("import-existing");
        write(&root, "area/page.md", "already here\n");
        let source = std::env::temp_dir().join(format!("factory-import-source-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(source.join("area")).unwrap();
        std::fs::write(source.join("area/page.md"), "new content\n").unwrap();

        let result = import(&root, &source, None, false).unwrap();
        assert!(result.skipped_existing.contains(&"area/page.md".to_string()));
        assert_eq!(std::fs::read_to_string(vault_root(&root).join("area/page.md")).unwrap(), "already here\n");

        let overwritten = import(&root, &source, None, true).unwrap();
        assert!(overwritten.copied.contains(&"area/page.md".to_string()));
        assert_eq!(std::fs::read_to_string(vault_root(&root).join("area/page.md")).unwrap(), "new content\n");

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&source).ok();
    }

    #[test]
    fn import_refuses_a_source_under_the_roots_own_secrets_or_factory_dir() {
        let root = temp_root("import-refuses-source");
        let secret_source = root.join("data/secrets");
        std::fs::create_dir_all(&secret_source).unwrap();
        std::fs::write(secret_source.join("x.md"), "nope").unwrap();

        let err = import(&root, &secret_source, None, false).unwrap_err();
        assert!(!err.contains("secrets/x"), "{err}");
    }

    #[test]
    fn import_ends_truncated_past_the_file_cap_keeping_what_it_already_copied() {
        let root = temp_root("import-truncated");
        let source = std::env::temp_dir().join(format!("factory-import-source-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&source).unwrap();
        for i in 0..(MAX_IMPORT_FILES + 5) {
            std::fs::write(source.join(format!("f{i:05}.md")), "x").unwrap();
        }

        let result = import(&root, &source, None, false).unwrap();
        assert!(result.truncated);
        assert_eq!(result.copied.len(), MAX_IMPORT_FILES);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&source).ok();
    }

    #[test]
    fn add_defaults_md_to_the_vault_root_and_everything_else_to_documents() {
        let root = temp_root("add-defaults");
        let source = std::env::temp_dir().join(format!("factory-add-source-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("note.md"), "---\ntitle: Note\n---\nBody.\n").unwrap();
        std::fs::write(source.join("scan.png"), "not a real png").unwrap();

        let result = add(&root, &[source.join("note.md"), source.join("scan.png")], None, false);
        assert!(result.copied.contains(&"note.md".to_string()), "{:?}", result.copied);
        assert!(result.copied.contains(&"documents/scan.png".to_string()), "{:?}", result.copied);
        assert!(vault_root(&root).join("note.md").exists());
        assert!(vault_root(&root).join("documents/scan.png").exists());

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&source).ok();
    }

    #[test]
    fn write_bytes_refuses_an_existing_target_without_overwrite_and_writes_with_it() {
        let root = temp_root("write-bytes");
        let first = write_bytes(&root, "documents/upload.bin", false, b"hello").unwrap();
        assert_eq!(first, "documents/upload.bin");
        assert!(write_bytes(&root, "documents/upload.bin", false, b"again").is_err());
        write_bytes(&root, "documents/upload.bin", true, b"again").unwrap();
        assert_eq!(std::fs::read(vault_root(&root).join("documents/upload.bin")).unwrap(), b"again");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn write_bytes_refuses_a_target_that_escapes_the_vault() {
        let root = temp_root("write-bytes-escape");
        let err = write_bytes(&root, "../escape.md", false, b"x").unwrap_err();
        assert!(err.contains("escapes"));

        std::fs::remove_dir_all(&root).ok();
    }
}
