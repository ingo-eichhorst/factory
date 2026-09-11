//! The shared knowledge note graph (design §7, backlog §12; ADR 0022).
//!
//! A note is one Markdown file. Its filename — [`NoteName`] — is its stable
//! identifier and the target other notes name with `[[note-name]]` in their
//! body. There is no separate id and no rename (ADR 0022 decision 6): the
//! filename *is* the link target, so changing it would silently break every
//! `[[link]]` pointing at the old name.
//!
//! This module owns four things: the name rule ([`NoteName::parse`]), the
//! frontmatter and body validation that makes a [`Note`] ([`Note::new`],
//! [`Note::parse`]), the atomic write both this module and [`crate::memory`]
//! use ([`Staged`], ADR 0022 decision 11), and the read-only graph view
//! ([`index`]) that rereads the note files rather than trusting any stored
//! table — there is no link table and this crate writes no index to disk.

use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// A note's stable identifier and its filename, minus the `.md` extension.
///
/// One to sixty-four characters, lowercase ASCII letters, digits, and
/// hyphens only; no leading or trailing hyphen; no two hyphens in a row
/// (ADR 0022 decision 6). The field is private so a `NoteName` can only ever
/// come from [`NoteName::parse`] — nothing downstream (rendering, staging,
/// linking) needs to re-check the rule, because a `NoteName` that exists is
/// proof it already passed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoteName(String);

impl NoteName {
    /// Refuses anything that is not decision 6's name shape, naming the
    /// rule in the error rather than just rejecting silently — an operator
    /// reading this message needs to know what to type instead.
    pub fn parse(raw: &str) -> Result<Self, NoteError> {
        let len = raw.chars().count();
        let charset_ok = raw
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        let valid = (1..=64).contains(&len)
            && charset_ok
            && !raw.starts_with('-')
            && !raw.ends_with('-')
            && !raw.contains("--");
        if valid {
            Ok(Self(raw.to_string()))
        } else {
            Err(NoteError::InvalidName(raw.to_string()))
        }
    }

    /// The name as it appears in a `[[link]]` and on disk (without `.md`).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The frontmatter fields a note must carry. See [`Note::new`] for what
/// "must" means precisely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub title: String,
    pub status: String,
    pub updated: String,
    pub sources: Vec<String>,
}

/// One note: its name, its frontmatter, and its body.
///
/// Every `Note` in existence has already passed [`Note::new`] or
/// [`Note::parse`]'s validation — there is no other constructor, and the
/// fields are `pub` for reading, not for building a `Note` around the
/// checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub name: NoteName,
    pub frontmatter: Frontmatter,
    pub body: String,
}

/// Judgement call (ADR 0022 decision 3's rule 3, "no source path lies under
/// `data/secrets/`"): refuse whenever `data` is immediately followed by
/// `secrets` anywhere in the path's components, not only when the path
/// begins that way.
///
/// - `data/secrets/x` — refused. The plain case.
/// - `./data/secrets/x` — refused. A leading `./` names the same location;
///   `.` components are dropped before comparing so this is not a way to
///   dodge the check by adding a no-op prefix.
/// - `/abs/data/secrets/x` — refused. If rooting the same two-component
///   pair absolutely were enough to pass, the rule would be decorative:
///   anyone could defeat it by writing an absolute path instead of a
///   relative one.
/// - `data/secrets` itself, with nothing after it — refused. The directory
///   is exactly as secret as anything inside it.
///
/// The cost of this breadth is a source whose path happens to contain an
/// unrelated `data` directory immediately followed by a `secrets` directory
/// somewhere in the middle of an unrelated absolute path. That false
/// positive is far cheaper than the false negative of a secret slipping
/// through because it was written down differently.
///
/// This function does not resolve `..` — resolving needs the filesystem,
/// and the source may not exist locally to resolve against. It looks at
/// the literal component sequence only, which is why [`validate`] refuses
/// any source containing a `..` component *before* this function ever
/// runs (see [`has_parent_reference`]): `data/foo/../secrets/key.pem`
/// splits into `[data, foo, .., secrets]`, no adjacent pair here is
/// `(data, secrets)`, and this function alone would wave it through even
/// though the path is under `data/secrets/` once resolved. That gap is
/// closed by refusing `..` outright, not by teaching this function to
/// resolve it.
fn is_secret_source(source: &str) -> bool {
    let components: Vec<&str> = source
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    components
        .windows(2)
        .any(|w| w[0] == "data" && w[1] == "secrets")
}

/// True if `source` contains a literal `..` path component.
///
/// [`validate`] refuses any source this matches, unconditionally — not
/// only ones that would otherwise reach into `data/secrets/`. A source
/// that needs `..` to say where it lives can be rewritten as a plain
/// relative or absolute path instead; refusing it outright is simpler and
/// safer than trying to decide, without resolving anything, which uses of
/// `..` are "fine" and which reach somewhere they shouldn't.
fn has_parent_reference(source: &str) -> bool {
    source.split('/').any(|c| c == "..")
}

/// The one place both [`Note::new`] and [`Note::parse`] check the rules —
/// so a note assembled from parts and a note read back off disk are held to
/// exactly the same standard.
fn validate(frontmatter: &Frontmatter, body: &str) -> Result<(), NoteError> {
    if frontmatter.title.is_empty() {
        return Err(NoteError::MissingField("title"));
    }
    if frontmatter.status.is_empty() {
        return Err(NoteError::MissingField("status"));
    }
    if frontmatter.updated.is_empty() {
        return Err(NoteError::MissingField("updated"));
    }
    // Judgement call: "an empty-string field counts as missing" (decision
    // 2) is written for the scalar fields, but the same gap exists for
    // `sources` — `sources: [""]` is zero real sources wearing a
    // non-empty `Vec`. Treating "every entry is blank" the same as "no
    // entries at all" closes that gap; a `Vec` with one blank entry
    // alongside a real one is left alone; the blank one is harmless noise,
    // not a hole in this rule.
    if frontmatter.sources.iter().all(|s| s.trim().is_empty()) {
        return Err(NoteError::MissingField("sources"));
    }

    // Judgement call: none of decision 2's four completeness rules says a
    // field cannot contain a newline, but `render`/`parse` below are a
    // fixed-position, one-field-per-line format — a newline inside a
    // scalar field would silently corrupt the file it produces, not
    // refuse cleanly. Rejecting it here is what makes `parse(render(n)) ==
    // n` actually hold for every `Note` that can exist, not just the ones
    // a test happened to try.
    for (field, value) in [
        ("title", frontmatter.title.as_str()),
        ("status", frontmatter.status.as_str()),
        ("updated", frontmatter.updated.as_str()),
    ] {
        if value.contains('\n') {
            return Err(NoteError::FieldNotSingleLine(field));
        }
    }

    for (i, source) in frontmatter.sources.iter().enumerate() {
        if source.contains('\n') {
            return Err(NoteError::FieldNotSingleLine("sources"));
        }
        if has_parent_reference(source) {
            return Err(NoteError::SourceTraversal { index: i + 1 });
        }
        if is_secret_source(source) {
            return Err(NoteError::SecretSource { index: i + 1 });
        }
    }

    // Decision 4: whitespace-only is refused along with truly empty, so a
    // body of `"\n"` (a pipe that produced nothing but its own line
    // ending) is caught the same as `""`.
    if body.trim().is_empty() {
        return Err(NoteError::EmptyBody);
    }

    Ok(())
}

impl Note {
    /// Build and validate a note from its parts. This is what a write path
    /// (CLI flags plus a stdin body, per decision 4) uses before ever
    /// touching disk.
    pub fn new(name: NoteName, frontmatter: Frontmatter, body: String) -> Result<Self, NoteError> {
        validate(&frontmatter, &body)?;
        Ok(Self {
            name,
            frontmatter,
            body,
        })
    }

    /// Parse a note file's full text back into a `Note`, holding it to the
    /// same rules [`Note::new`] enforces — a note read off disk is never
    /// trusted more than one just built from parts.
    pub fn parse(name: NoteName, text: &str) -> Result<Self, NoteError> {
        let (frontmatter, body) = parse_frontmatter(text)?;
        validate(&frontmatter, &body)?;
        Ok(Self {
            name,
            frontmatter,
            body,
        })
    }

    /// The exact bytes that go on disk. Fixed field order (title, status,
    /// updated, sources), one field per line, so a byte-for-byte identical
    /// `Note` produces byte-for-byte identical output — `parse(render(n))
    /// == n` (proven in this crate's tests), because `str::split('\n')`
    /// and `[&str]::join("\n")` are exact inverses of each other and every
    /// scalar field is checked newline-free by [`validate`].
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("title: {}\n", self.frontmatter.title));
        out.push_str(&format!("status: {}\n", self.frontmatter.status));
        out.push_str(&format!("updated: {}\n", self.frontmatter.updated));
        out.push_str("sources:\n");
        for source in &self.frontmatter.sources {
            out.push_str(&format!("  - {source}\n"));
        }
        out.push_str("---\n");
        out.push('\n');
        out.push_str(&self.body);
        out
    }

    /// Every `[[target]]` in the body, in order of first appearance,
    /// deduplicated. A target is whatever text sits between the double
    /// brackets, taken verbatim — no trimming, no alias syntax. Whether it
    /// names a real note is [`index`]'s question, not this one: a link
    /// here is just text the author wrote.
    pub fn links(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut rest = self.body.as_str();
        while let Some(start) = rest.find("[[") {
            let after_open = &rest[start + 2..];
            let Some(end) = after_open.find("]]") else {
                break; // an unterminated `[[` ends scanning; nothing after it is a link
            };
            let target = &after_open[..end];
            if seen.insert(target.to_string()) {
                out.push(target.to_string());
            }
            rest = &after_open[end + 2..];
        }
        out
    }
}

/// Parse the frontmatter shape [`Note::render`] writes:
///
/// ```text
/// ---
/// title: <value>
/// status: <value>
/// updated: <value>
/// sources:
///   - <value>
///   - <value>
/// ---
///
/// <body>
/// ```
///
/// The four fields are accepted in **any** order — reordering two lines is
/// the most ordinary edit a person makes to a note, and a parser that
/// refused a note for that would take the whole [`index`] down with it
/// (before this crate's rework, it did: see [`Index::unreadable`]'s doc
/// comment). [`Note::render`] still always writes them in the fixed order
/// above, so `parse(render(n)) == n` continues to hold exactly — this
/// function does not need to produce that order back, only to accept it,
/// along with every other order.
///
/// A duplicate field (the same key twice) and a field missing outright
/// (its line never appears at all) are both refused as
/// [`NoteError::MalformedFrontmatter`], the same as before this rework. A
/// field that *appears* but is empty (`title: `) is not caught here —
/// that is [`validate`]'s [`NoteError::MissingField`], a semantic
/// question this purely structural reader does not ask.
///
/// A line that is not `title: `, `status: `, `updated: `, `sources:`, or
/// `---` is refused the same way. This function does not accept a key it
/// does not know: whether `knowledge write` should ever let a caller add
/// its own custom frontmatter keys is a real design question this
/// function deliberately does not settle by defaulting to "yes".
///
/// This is a position-*tolerant*, not a general YAML, reader — there is no
/// YAML dependency in this crate, and every shape this function accepts is
/// still one [`Note::render`] could have produced, just reordered.
fn parse_frontmatter(text: &str) -> Result<(Frontmatter, String), NoteError> {
    // `text.split('\n')` and `<[&str]>::join("\n")` are exact inverses, so
    // splitting into lines here and rejoining the body's tail below loses
    // no information — not even a trailing newline — which is what makes
    // the render/parse round trip exact rather than approximate.
    let lines: Vec<&str> = text.split('\n').collect();
    let mut i = 0usize;

    if lines.first().copied() != Some("---") {
        return Err(NoteError::MalformedFrontmatter(
            "missing opening `---` line".to_string(),
        ));
    }
    i += 1;

    let mut title: Option<String> = None;
    let mut status: Option<String> = None;
    let mut updated: Option<String> = None;
    let mut sources: Option<Vec<String>> = None;

    loop {
        match lines.get(i).copied() {
            Some("---") => {
                i += 1;
                break;
            }
            Some(line) if line.starts_with("title: ") => {
                if title.is_some() {
                    return Err(NoteError::MalformedFrontmatter(
                        "duplicate `title` field".to_string(),
                    ));
                }
                title = Some(line["title: ".len()..].to_string());
                i += 1;
            }
            Some(line) if line.starts_with("status: ") => {
                if status.is_some() {
                    return Err(NoteError::MalformedFrontmatter(
                        "duplicate `status` field".to_string(),
                    ));
                }
                status = Some(line["status: ".len()..].to_string());
                i += 1;
            }
            Some(line) if line.starts_with("updated: ") => {
                if updated.is_some() {
                    return Err(NoteError::MalformedFrontmatter(
                        "duplicate `updated` field".to_string(),
                    ));
                }
                updated = Some(line["updated: ".len()..].to_string());
                i += 1;
            }
            Some("sources:") => {
                if sources.is_some() {
                    return Err(NoteError::MalformedFrontmatter(
                        "duplicate `sources` field".to_string(),
                    ));
                }
                i += 1;
                let mut list = Vec::new();
                while let Some(line) = lines.get(i).and_then(|l| l.strip_prefix("  - ")) {
                    list.push(line.to_string());
                    i += 1;
                }
                sources = Some(list);
            }
            Some(_) => {
                return Err(NoteError::MalformedFrontmatter(
                    "unrecognised line in frontmatter".to_string(),
                ));
            }
            None => {
                return Err(NoteError::MalformedFrontmatter(
                    "missing closing `---` line".to_string(),
                ));
            }
        }
    }

    let title = title
        .ok_or_else(|| NoteError::MalformedFrontmatter("missing `title` field".to_string()))?;
    let status = status
        .ok_or_else(|| NoteError::MalformedFrontmatter("missing `status` field".to_string()))?;
    let updated = updated
        .ok_or_else(|| NoteError::MalformedFrontmatter("missing `updated` field".to_string()))?;
    let sources = sources
        .ok_or_else(|| NoteError::MalformedFrontmatter("missing `sources` field".to_string()))?;

    if lines.get(i).copied() != Some("") {
        return Err(NoteError::MalformedFrontmatter(
            "expected a blank line after frontmatter".to_string(),
        ));
    }
    i += 1;

    let body = lines[i..].join("\n");

    Ok((
        Frontmatter {
            title,
            status,
            updated,
            sources,
        },
        body,
    ))
}

/// A filesystem-unique suffix for a temporary file, without pulling in
/// `uuid`'s `v4` feature — this workspace pins `uuid = "1"` with no extra
/// features (see `factory-delegation`'s tests, which build UUIDs by hand
/// for the same reason), and this crate's `Cargo.toml` is not the place to
/// widen that. Process id plus a monotonic in-process counter plus a
/// nanosecond timestamp is unique enough for a name nobody ever looks up —
/// it only has to not collide with another temp file in the same
/// directory, never to be guessed or compared.
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{pid}-{nanos}-{n}")
}

/// A file staged in place, waiting to be made visible under its real name.
///
/// [`stage`] and [`crate::memory::stage_entry`] both build one of these —
/// ADR 0022 decision 11: the write-then-rename primitive is the same for
/// both, and written twice it would be correct in one place and nearly
/// correct in the other. Neither caller's collision policy lives here: a
/// note may be overwritten when `--update` is given, a memory entry never
/// is, and each caller decides that for itself before calling
/// [`Staged::create`] — this type only knows how to make bytes appear
/// somewhere without a half-written state in between.
#[derive(Debug)]
pub struct Staged {
    tmp_path: PathBuf,
    final_path: PathBuf,
    committed: bool,
}

impl Staged {
    /// Write `contents` to a temporary file in `dir` — the *destination*
    /// directory, deliberately not `std::env::temp_dir()` — and flush it
    /// to disk. Nothing is visible under `final_path` yet; that only
    /// happens in [`Staged::commit`].
    ///
    /// The temp file's name carries no `.md` suffix on purpose: `index`
    /// only looks at files whose extension is `md`, so a temp file
    /// orphaned by a crash between here and `commit` (nothing ran its
    /// `Drop`) is invisible to every reader in this crate, not merely to
    /// the ones that happen to skip dot-files.
    ///
    /// Staying in `dir` is what makes [`Staged::commit`]'s rename atomic:
    /// a rename is only guaranteed atomic when both paths are on the same
    /// filesystem, and a mount point boundary is exactly what putting the
    /// temp file in the system temp directory would risk.
    pub(crate) fn create(
        dir: &Path,
        final_path: PathBuf,
        contents: &[u8],
    ) -> Result<Self, NoteError> {
        let tmp_path = dir.join(format!(".tmp-{}", unique_suffix()));
        let mut file = std::fs::File::create(&tmp_path)?;
        file.write_all(contents)?;
        file.sync_all()?; // flushed to disk before any caller is told staging succeeded
        Ok(Self {
            tmp_path,
            final_path,
            committed: false,
        })
    }

    /// Where the file will land once committed — not where it sits right
    /// now.
    pub fn path(&self) -> &Path {
        &self.final_path
    }

    /// Make the staged file visible under its real name. A single
    /// `rename` syscall on one filesystem is atomic, so there is no
    /// instant at which a reader could observe a truncated or partial
    /// file at `final_path`: it is either still whatever it was before
    /// (nonexistent, or the old note) or it is entirely the new content —
    /// never a mixture.
    ///
    /// This is the half of ADR 0022 decision 3 that visibility alone does
    /// not satisfy — see [`sync_parent_dir`]'s doc comment for why the
    /// directory is fsynced here too, not just the file back in
    /// [`Staged::create`].
    pub fn commit(mut self) -> Result<PathBuf, NoteError> {
        std::fs::rename(&self.tmp_path, &self.final_path)?;
        self.committed = true;
        sync_parent_dir(&self.final_path)?;
        Ok(self.final_path.clone())
    }
}

/// Fsync the directory `path` lives in.
///
/// ADR 0022 decision 3 turns on which failure a crash right after the
/// caller's own `tx.commit()` can leave behind: a note with no provenance
/// row (missing — tolerable, per ADR 0017 and decision 3's own argument),
/// or a provenance row asserting a note that does not exist (wrong — the
/// thing decision 3 exists to rule out). [`Staged::commit`]'s `rename`
/// makes the new directory entry *visible* to anyone who looks right
/// after it returns, but visible is not durable: on every mainstream
/// filesystem this crate runs on, a directory's own entries can sit
/// unwritten to the device until something calls `fsync` on the directory
/// itself, separately from `fsync`ing the file. Skipping this call would
/// make decision 3's argument false in exactly the case it exists to
/// cover: a power failure between `rename` returning and the caller's own
/// `tx.commit()` could make the rename evaporate on the next boot even
/// though the row asserting it already landed durably.
///
/// This call is argued here, not proven by this crate's test suite.
/// Nothing in `tests/` demonstrates durability across a real power loss —
/// a unit test cannot pull power from a disk mid-`fsync` and then inspect
/// what survived. A green test suite with this call present is not
/// evidence the call matters, and a green test suite with it removed
/// (which the crate's own mutation report does, and reverts) is not
/// evidence it doesn't: no test here can tell the difference, by
/// construction. Do not read either result as settling the question the
/// comment above is settling by argument instead.
///
/// Opening a directory with `File::open` purely to call `sync_all` on it
/// is POSIX behaviour — this crate's workspace targets macOS and Linux.
/// It is not portable to Windows, where opening a directory as a file is
/// refused; this crate does not attempt to special-case that platform.
fn sync_parent_dir(path: &Path) -> Result<(), NoteError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

impl Drop for Staged {
    /// A `Staged` that is dropped without `commit` — the caller changed
    /// its mind, or something failed after staging but before the store
    /// transaction that was going to commit it — leaves no temporary file
    /// behind. Best-effort: if the file is already gone, or removal fails
    /// for some other reason, there is no further error path to report it
    /// through from inside `drop`.
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.tmp_path);
        }
    }
}

/// The file a note named `name` occupies in `dir`. One Markdown file per
/// note, named after [`NoteName::as_str`] plus `.md` — this is the one
/// function that says so; [`stage`], [`read`], and [`index`] all go
/// through it rather than each spelling out the extension.
fn note_path(dir: &Path, name: &NoteName) -> PathBuf {
    dir.join(format!("{}.md", name.as_str()))
}

/// Stage `note` for writing into `dir`. Nothing under the note's real name
/// changes until the returned [`Staged`] is committed.
///
/// Refuses with [`NoteError::AlreadyExists`] when a note already exists
/// under this name and `update` is `false` (decision 5) — the refusal
/// names the note and points at `--update`, the flag that turns an
/// accidental overwrite into one the caller asked for.
pub fn stage(dir: &Path, note: &Note, update: bool) -> Result<Staged, NoteError> {
    let final_path = note_path(dir, &note.name);
    if !update && final_path.exists() {
        return Err(NoteError::AlreadyExists(note.name.as_str().to_string()));
    }
    Staged::create(dir, final_path, note.render().as_bytes())
}

/// Read the note named `name` back out of `dir`.
pub fn read(dir: &Path, name: &NoteName) -> Result<Note, NoteError> {
    let path = note_path(dir, name);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            NoteError::NotFound(name.as_str().to_string())
        } else {
            NoteError::Io(e)
        }
    })?;
    Note::parse(name.clone(), &text)
}

/// One note's place in the graph, as [`index`] sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub name: NoteName,
    pub title: String,
    /// Outbound links, exactly as [`Note::links`] returned them for this
    /// note — in order of first appearance, not sorted, because this is
    /// what the author wrote and reordering it would lose that.
    pub links: Vec<String>,
    /// Every other note in `dir` whose body links to this one, sorted by
    /// name for determinism.
    pub backlinks: Vec<NoteName>,
}

/// The whole note graph in `dir`, rebuilt by rereading the files — there is
/// no cache and no index file, so calling this twice on unchanged notes is
/// the only way to get the answer twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    /// Sorted by [`NoteName`], not by directory-read order — `read_dir`'s
    /// order is not guaranteed by any filesystem this crate runs on, and
    /// determinism has to come from an explicit sort, not from an
    /// assumption about the OS.
    pub notes: Vec<IndexEntry>,
    /// `(source note, link target)` pairs for every `[[link]]` that names
    /// no note in `dir`. This is not an error (see [`index`]'s doc
    /// comment) — it is the answer for a link an author wrote before its
    /// target existed, or that never will.
    pub unresolved: Vec<(NoteName, String)>,
    /// `(filename, reason)` for a `*.md` file whose name is a valid
    /// [`NoteName`] but whose content could not be read or parsed. Also
    /// not an error, for the same reason `unresolved` is not one — see
    /// [`index`]'s doc comment. Sorted by filename for determinism, the
    /// same reason [`Self::notes`] is sorted rather than left in
    /// directory-read order.
    pub unreadable: Vec<(String, String)>,
}

/// Rebuild the note graph in `dir` by reading every `*.md` file there.
///
/// A `[[link]]` to a name with no matching note is **not** an error. It is
/// reported in [`Index::unresolved`]. This is the single most important
/// behaviour of this function: an index that refused on a dangling link
/// would refuse to show anything the moment one author's note outran
/// another's, which is the ordinary, unremarkable state of a wiki that is
/// still being written.
///
/// A file whose name is not a valid [`NoteName`], or that carries no `.md`
/// extension, is not a note this crate ever wrote — it is skipped
/// entirely and appears nowhere in the returned [`Index`], not even in
/// [`Index::unreadable`] (a stray `.gitkeep`, a leftover `.tmp-*` from a
/// crash between [`Staged::create`] and `commit`, both fall out of the
/// graph this way, the second one because its name carries no `.md`
/// extension at all).
///
/// A `*.md` file whose name *is* a valid [`NoteName`] but whose content
/// cannot be read or parsed is a different case, and — like an unresolved
/// link — is **not** refused either: it is reported in
/// [`Index::unreadable`] instead, named by its filename and the reason,
/// and every other note in `dir` still comes back. Returning `Err` here
/// used to mean one corrupt file took the whole call down with it, so
/// `knowledge list` reported nothing at all — no notes, no backlinks, no
/// unresolved links — for a problem that affects exactly one file. That is
/// a worse failure than refusing on a dangling link would have been, for
/// the same reason: a reader who wants "did everything parse" checks
/// whether `unreadable` is empty, and a reader who wants "show me what
/// there is" gets everything else regardless.
///
/// A note that could not be loaded contributes nothing to `existing`
/// below, so a `[[link]]` to it from some other note lands in
/// `unresolved`, not `unreadable` — the linking note did nothing wrong;
/// the broken note is where the problem is recorded.
pub fn index(dir: &Path) -> Result<Index, NoteError> {
    let mut loaded: Vec<(NoteName, Note)> = Vec::new();
    let mut unreadable: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(name) = NoteName::parse(stem) else {
            continue;
        };
        let filename = path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(stem)
            .to_string();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                unreadable.push((filename, e.to_string()));
                continue;
            }
        };
        match Note::parse(name.clone(), &text) {
            Ok(note) => loaded.push((name, note)),
            Err(e) => unreadable.push((filename, e.to_string())),
        }
    }
    loaded.sort_by(|(a, _), (b, _)| a.cmp(b));
    unreadable.sort();

    let existing: HashSet<&str> = loaded.iter().map(|(name, _)| name.as_str()).collect();

    let mut backlinks: BTreeMap<String, Vec<NoteName>> = BTreeMap::new();
    let mut unresolved: Vec<(NoteName, String)> = Vec::new();
    for (name, note) in &loaded {
        for link in note.links() {
            if existing.contains(link.as_str()) {
                backlinks.entry(link).or_default().push(name.clone());
            } else {
                unresolved.push((name.clone(), link));
            }
        }
    }
    for names in backlinks.values_mut() {
        names.sort();
    }
    unresolved.sort_by(|a, b| (a.0.as_str(), a.1.as_str()).cmp(&(b.0.as_str(), b.1.as_str())));

    let notes = loaded
        .iter()
        .map(|(name, note)| IndexEntry {
            name: name.clone(),
            title: note.frontmatter.title.clone(),
            links: note.links(),
            backlinks: backlinks.get(name.as_str()).cloned().unwrap_or_default(),
        })
        .collect();

    Ok(Index {
        notes,
        unresolved,
        unreadable,
    })
}

/// Every way building, staging, reading, or indexing a note can be
/// refused.
///
/// Reused as [`crate::memory`]'s error type too (its public functions
/// return `Result<_, NoteError>` — see that module's docs): every failure
/// a memory write or read can hit is already one of the variants below —
/// an I/O error, a collision [`Staged::create`] must refuse, or an empty
/// entry with the same shape as [`NoteError::EmptyBody`]'s reasoning — so a
/// second, `MemoryError` type would exist only to wrap this one.
#[derive(Debug, thiserror::Error)]
pub enum NoteError {
    #[error(
        "note name {0:?} is invalid\n  help: a name is 1-64 characters of lowercase ascii letters, digits, and hyphens, with no leading or trailing hyphen and no two hyphens in a row"
    )]
    InvalidName(String),

    #[error(
        "note frontmatter is missing its {0} field\n  help: title, status, updated, and at least one source are all required; an empty string counts as missing"
    )]
    MissingField(&'static str),

    #[error(
        "source #{index} lies under data/secrets/\n  help: a note must not carry a path into the secrets tree — point the source at a non-secret copy or drop it from the list"
    )]
    SecretSource { index: usize },

    #[error(
        "source #{index} contains a `..` component\n  help: a source path must not walk upward — rewrite it as a plain relative or absolute path"
    )]
    SourceTraversal { index: usize },

    #[error(
        "note body is empty\n  help: an empty (or whitespace-only) body is indistinguishable from a failed pipe — write the note's content to stdin"
    )]
    EmptyBody,

    #[error(
        "frontmatter field {0} cannot span more than one line\n  help: frontmatter is one field per line; put multi-line content in the body"
    )]
    FieldNotSingleLine(&'static str),

    #[error("note text is not in the expected frontmatter format: {0}")]
    MalformedFrontmatter(String),

    #[error("note {0} already exists\n  help: pass --update to overwrite it")]
    AlreadyExists(String),

    #[error("note {0} does not exist")]
    NotFound(String),

    #[error(
        "memory scope {0:?} is invalid\n  help: a scope is a single path segment — it must not be empty, contain '/', or contain '..'"
    )]
    InvalidScope(String),

    #[error(
        "memory entry is empty\n  help: an empty (or whitespace-only) entry is indistinguishable from a failed pipe"
    )]
    EmptyEntry,

    #[error(
        "memory entry timestamp {0:?} is invalid\n  help: created_at must be non-empty and must not contain '/'"
    )]
    InvalidCreatedAt(String),

    #[error(
        "memory entry {0} already exists\n  help: entries are never overwritten — this id was already used"
    )]
    DuplicateEntry(String),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}
