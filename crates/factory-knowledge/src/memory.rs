//! Scope-local memory (design §7, backlog §12; ADR 0022).
//!
//! Where [`crate::note`] is shared and sourced, memory is scope-local and
//! unsourced — design §7's split, kept at the surface (ADR 0022 decision
//! 11). One file per entry under `<memory_root>/<scope>/`. There is no
//! frontmatter here: an entry is not a note that other entries link to, it
//! is a scratch line a scope leaves for its future self, so the file holds
//! exactly the entry text and nothing else.
//!
//! This module has no `Staged` or error type of its own. It reuses
//! [`crate::note::Staged`] and [`crate::note::NoteError`] — ADR 0022
//! decision 11's "written twice it would be correct in one place and
//! nearly correct in the other" applies to the write primitive; it applies
//! just as much to not inventing a second near-identical error enum next
//! to the first.

use std::path::Path;

use crate::note::{NoteError, Staged};

// `Staged::create` is `pub(crate)`, not `pub` — it takes no position on
// collision policy (see `Staged`'s own doc comment), so every caller,
// including this module, must have already decided that for itself before
// reaching it. That decision is made just above each call site below.

/// One entry read back by [`list_entries`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEntry {
    pub id: uuid::Uuid,
    pub created_at: String,
    pub text: String,
}

/// A scope is exactly one path segment directly under `memory_root`.
///
/// Rejecting any `/` already rejects every absolute path too (an absolute
/// path starts with `/`), so there is no separate "is this absolute" check
/// to write, or to forget. `..` is rejected as a substring, not by
/// resolving the path and comparing it to `memory_root` — resolving it
/// would be normalising it, which decision-adjacent guidance for this
/// module explicitly asks not to do: a scope is either a bare safe segment
/// on its face, or it is refused outright, with no attempt to guess what a
/// caller "really meant".
fn validate_scope(scope: &str) -> Result<(), NoteError> {
    if scope.is_empty() || scope.contains('/') || scope.contains("..") {
        return Err(NoteError::InvalidScope(scope.to_string()));
    }
    Ok(())
}

/// The directory one scope's entries live in, and the filename one entry
/// occupies within it.
///
/// `created_at` is the filename's sortable prefix — [`list_entries`] still
/// sorts explicitly rather than trusting directory order, but naming files
/// this way means a plain directory listing is *also* roughly
/// chronological, which is worth having for an operator poking around with
/// `ls`. `id` is the suffix, joined with `--`: a UUID's canonical string
/// form never contains `--` (each group is separated by a single hyphen),
/// so splitting from the right on the last `--` finds exactly this
/// boundary regardless of what `created_at` itself contains — as long as
/// `created_at` has no `/` in it, which [`stage_entry`] already checks
/// before it ever reaches here.
fn entry_path(
    memory_root: &Path,
    scope: &str,
    created_at: &str,
    id: uuid::Uuid,
) -> std::path::PathBuf {
    memory_root
        .join(scope)
        .join(format!("{created_at}--{id}.md"))
}

/// Stage one memory entry for `scope`. Nothing is visible under the
/// entry's real name until the returned [`Staged`] is committed.
///
/// An entry is create-only — there is no `update` parameter, unlike
/// [`crate::note::stage`], because a memory entry is never revised: it is
/// refused with [`NoteError::DuplicateEntry`] if its computed filename
/// already exists (in ordinary use this cannot happen, since `id` is
/// expected to be freshly generated per entry; the check exists so that a
/// caller retrying with the same `id` gets a refusal instead of silently
/// rewriting the earlier entry).
///
/// Refuses an empty or whitespace-only `entry` with
/// [`NoteError::EmptyEntry`] — the same reasoning as a note's empty body:
/// indistinguishable from a failed pipe.
///
/// `<memory_root>/<scope>/` is created if it does not exist yet — unlike
/// `crate::note::stage`'s `dir`, which is one fixed directory a caller
/// sets up ahead of time, a scope's directory is one of arbitrarily many,
/// created the first time that scope writes an entry.
pub fn stage_entry(
    memory_root: &Path,
    scope: &str,
    entry: &str,
    id: uuid::Uuid,
    created_at: &str,
) -> Result<Staged, NoteError> {
    validate_scope(scope)?;
    if entry.trim().is_empty() {
        return Err(NoteError::EmptyEntry);
    }
    if created_at.is_empty() || created_at.contains('/') {
        return Err(NoteError::InvalidCreatedAt(created_at.to_string()));
    }

    let scope_dir = memory_root.join(scope);
    std::fs::create_dir_all(&scope_dir)?;

    let final_path = entry_path(memory_root, scope, created_at, id);
    if final_path.exists() {
        return Err(NoteError::DuplicateEntry(id.to_string()));
    }

    Staged::create(&scope_dir, final_path, entry.as_bytes())
}

/// Every entry recorded for `scope`, sorted by `created_at` then `id` —
/// never by directory-read order, which no filesystem this crate runs on
/// guarantees.
///
/// A scope with no entries yet (its directory does not exist) returns an
/// empty list rather than an error: "nobody has written to this scope" is
/// not a failure.
pub fn list_entries(memory_root: &Path, scope: &str) -> Result<Vec<MemoryEntry>, NoteError> {
    validate_scope(scope)?;
    let scope_dir = memory_root.join(scope);

    let read_dir = match std::fs::read_dir(&scope_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(NoteError::Io(e)),
    };

    let mut entries = Vec::new();
    for dir_entry in read_dir {
        let dir_entry = dir_entry?;
        let path = dir_entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // The last `--` is always the `created_at`/`id` boundary — see
        // `entry_path`'s doc comment for why that holds regardless of
        // what `created_at` contains.
        let Some((created_at, id_str)) = stem.rsplit_once("--") else {
            continue;
        };
        let Ok(id) = uuid::Uuid::parse_str(id_str) else {
            continue;
        };
        let text = std::fs::read_to_string(&path)?;
        entries.push(MemoryEntry {
            id,
            created_at: created_at.to_string(),
            text,
        });
    }

    entries.sort_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(entries)
}
