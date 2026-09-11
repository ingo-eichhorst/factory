//! `knowledge.write`, `knowledge.list`, `knowledge.show` (design §7, backlog
//! §12; ADR 0022).
//!
//! This module invents no rule of its own. `factory_knowledge::note` already
//! holds the name rule, the frontmatter validation, and the atomic
//! stage/commit primitive (ADR 0022 decision 11); this file's own job is
//! decision 3's five-step write order and nothing else:
//!
//! 1. [`factory_knowledge::note::stage`] — write and flush a temporary file,
//!    nothing visible yet.
//! 2. open the store transaction.
//! 3. [`factory_store::durable::append`] — the `durable_writes` row.
//! 4. [`factory_knowledge::note::Staged::commit`] — the rename.
//! 5. commit the transaction.
//!
//! That order is fixed, not a style choice: with the transaction committed
//! last, a crash between steps 4 and 5 leaves a note with no provenance row
//! (a missing observation — survivable, per ADR 0017), never a row asserting
//! a note that was never renamed into place (a wrong observation — the
//! failure this ordering exists to rule out). See `tests/knowledge.rs`'s own
//! mutation for the test that pins this ordering down.

use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::errors;
use crate::handler::FactoryHandler;
use crate::server::HandlerOutcome;

/// `<instance_root>/.factory/knowledge/` — the one place every function in
/// this module joins this path together, so `write` and `list`/`show`
/// cannot drift onto two different directories.
fn knowledge_dir(instance_root: &std::path::Path) -> std::path::PathBuf {
    instance_root.join(".factory").join("knowledge")
}

/// A path this crate itself just wrote or staged, rendered relative to the
/// instance root — `schema::V7_SCHEMA`'s own doc comment on
/// `durable_writes.path` says why: a row stays meaningful if the instance is
/// ever moved to a new absolute path. Falls back to the absolute path only
/// if `path` is somehow not under `instance_root` at all, which none of this
/// module's own callers can produce.
fn relative_to_instance(instance_root: &std::path::Path, path: &std::path::Path) -> String {
    path.strip_prefix(instance_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WritePayload {
    name: String,
    title: String,
    status: String,
    /// `--source`, repeated. At least one non-blank entry is required —
    /// [`factory_knowledge::note::Note::new`]'s own validation, not
    /// re-checked here.
    #[serde(default)]
    sources: Vec<String>,
    /// The note's body, read from standard input by the CLI (ADR 0022
    /// decision 4) and carried here as an ordinary field — this crate never
    /// reads a caller's stdin itself.
    body: String,
    /// `--update`. Without it, writing a name that already exists is
    /// refused (decision 5) — enforced by [`factory_knowledge::note::stage`]
    /// itself, not re-derived here.
    #[serde(default)]
    update: bool,
    /// `--task-id`, optional. Recorded on the `durable_writes` row so the
    /// write's provenance survives the session that made it.
    #[serde(default, with = "crate::serde_uuid::optional")]
    task_id: Option<uuid::Uuid>,
}

/// Decision 13: the daemon stamps `updated` at write time; the caller never
/// supplies it. `chrono::Utc::now()` is the same clock this crate already
/// uses for every other timestamp it produces (`ops::schedule`'s own
/// `Utc::now()` calls), rendered as RFC3339 so it sorts and parses the same
/// way every other daemon-produced timestamp does.
///
/// Second resolution, not `to_rfc3339`'s default microseconds: design §7
/// and backlog §12 both call this an "updated *date*," nothing anywhere
/// orders notes by sub-second time, and every other `created_at` this
/// system stamps is SQLite's `CURRENT_TIMESTAMP` — also second-resolution.
/// Sub-second digits in a note a person reads are noise that also disagrees
/// with the rest of the system, for no gain.
pub(crate) fn write(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    let payload: WritePayload = errors::parse_payload(&payload)?;

    let name =
        factory_knowledge::note::NoteName::parse(&payload.name).map_err(errors::knowledge_error)?;
    let updated = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let frontmatter = factory_knowledge::note::Frontmatter {
        title: payload.title,
        status: payload.status,
        updated: updated.clone(),
        sources: payload.sources,
    };
    let note = factory_knowledge::note::Note::new(name, frontmatter, payload.body)
        .map_err(errors::knowledge_error)?;

    let dir = knowledge_dir(h.instance_root());
    std::fs::create_dir_all(&dir).map_err(|e| errors::err("internal.io_error", e.to_string()))?;

    // Step 1: stage. Nothing under the note's real name changes yet.
    let staged = factory_knowledge::note::stage(&dir, &note, payload.update)
        .map_err(errors::knowledge_error)?;
    let relative_path = relative_to_instance(h.instance_root(), staged.path());

    let mut store = h.lock_store();
    // Step 2: open the transaction.
    let tx = store.transaction().map_err(errors::store_error)?;

    // Step 3: the provenance row, inside the still-open transaction.
    factory_store::durable::append(
        &tx,
        factory_store::durable::WriteKind::Knowledge,
        note.name.as_str(),
        &relative_path,
        None,
        payload.task_id.map(|id| id.to_string()).as_deref(),
        None,
    )
    .map_err(errors::store_error)?;

    // Step 4: the rename. If this fails, `tx` is dropped without ever being
    // committed — `rusqlite`'s own `Drop` rolls it back, so the row from
    // step 3 never becomes durable. See this module's own doc comment.
    let final_path = staged.commit().map_err(errors::knowledge_error)?;

    // Step 5: only now does the row become durable.
    tx.commit().map_err(errors::store_error)?;

    h.success(
        json!({
            "name": note.name.as_str(),
            "path": relative_to_instance(h.instance_root(), &final_path),
            "updated": updated,
        }),
        true,
    )
}

pub(crate) fn list(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {}
    let _: Payload = errors::parse_payload(&payload)?;

    let dir = knowledge_dir(h.instance_root());
    // No note has ever been written: an empty index, not a missing-directory
    // error — the same stance `factory_knowledge::memory::list_entries`
    // takes for a scope with no entries yet.
    if !dir.exists() {
        return h.success(
            json!({ "notes": Value::Array(vec![]), "unresolved": Value::Array(vec![]), "unreadable": Value::Array(vec![]) }),
            false,
        );
    }

    let index = factory_knowledge::note::index(&dir).map_err(errors::knowledge_error)?;

    let notes: Vec<Value> = index
        .notes
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name.as_str(),
                "title": entry.title,
                "links": entry.links,
                "backlinks": entry.backlinks.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
            })
        })
        .collect();

    // A dangling `[[link]]` is a gap, not an error (this module's own doc
    // comment on `factory_knowledge::note::index`'s doc comment says why) —
    // reported here, never refused.
    let unresolved: Vec<Value> = index
        .unresolved
        .iter()
        .map(|(from, target)| json!({ "from": from.as_str(), "target": target }))
        .collect();

    // An unreadable file is one file's problem, not the whole index's — it
    // must never silently disappear from this response (mutation 8's own
    // target).
    let unreadable: Vec<Value> = index
        .unreadable
        .iter()
        .map(|(filename, reason)| json!({ "filename": filename, "reason": reason }))
        .collect();

    h.success(
        json!({ "notes": notes, "unresolved": unresolved, "unreadable": unreadable }),
        false,
    )
}

/// `knowledge show --name <name>`: the note exactly as it is on disk. This
/// resolves no `[[link]]` and renders nothing (ADR 0022's own consequences
/// section) — `text` is [`factory_knowledge::note::Note::render`]'s output
/// verbatim, byte for byte what `knowledge write` produced.
pub(crate) fn show(h: &FactoryHandler, _scope_id: uuid::Uuid, payload: Value) -> HandlerOutcome {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Payload {
        name: String,
    }
    let payload: Payload = errors::parse_payload(&payload)?;

    let name =
        factory_knowledge::note::NoteName::parse(&payload.name).map_err(errors::knowledge_error)?;
    let dir = knowledge_dir(h.instance_root());
    let note = factory_knowledge::note::read(&dir, &name).map_err(errors::knowledge_error)?;

    h.success(
        json!({
            "name": note.name.as_str(),
            "title": note.frontmatter.title,
            "status": note.frontmatter.status,
            "updated": note.frontmatter.updated,
            "sources": note.frontmatter.sources,
            "body": note.body,
            "text": note.render(),
        }),
        false,
    )
}
