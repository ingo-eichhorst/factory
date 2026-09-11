//! `knowledge.write`, `knowledge.list`, `knowledge.show` (ADR 0022).

mod common;

use factory_daemon::envelope::{CommandRequest, QueryRequest};
use factory_daemon::{FactoryHandler, Handler};

fn uid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

fn cmd(command: &str, payload: serde_json::Value) -> CommandRequest {
    CommandRequest {
        request_id: uid(9700),
        scope_id: uuid::Uuid::nil(),
        command: command.to_string(),
        payload,
        expected_revision: None,
    }
}

fn qry(query: &str, payload: serde_json::Value) -> QueryRequest {
    QueryRequest {
        request_id: uid(9701),
        scope_id: uuid::Uuid::nil(),
        query: query.to_string(),
        payload,
    }
}

fn new_handler(fixture: &common::Fixture) -> FactoryHandler {
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let adapter = common::FakeAdapter::new();
    FactoryHandler::new(store, adapter, fixture.instance_root())
}

fn write_payload(name: &str, title: &str, body: &str, update: bool) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "title": title,
        "status": "draft",
        "sources": ["some-source.md"],
        "body": body,
        "update": update,
    })
}

fn durable_write_count(fixture: &common::Fixture, kind: &str, name: &str) -> i64 {
    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    store
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM durable_writes WHERE kind = ?1 AND name = ?2",
            [kind, name],
            |row| row.get(0),
        )
        .expect("count query succeeds")
}

#[test]
fn write_then_show_round_trips() {
    let fixture = common::build(&[]);
    let handler = new_handler(&fixture);

    let written = handler
        .handle_command(cmd(
            "knowledge.write",
            write_payload("my-note", "My Note", "the body text", false),
        ))
        .expect("knowledge.write succeeds");
    assert_eq!(written.result["name"], "my-note");
    let updated = written.result["updated"]
        .as_str()
        .expect("updated is a string");
    chrono::DateTime::parse_from_rfc3339(updated).expect("updated is RFC3339");
    // Station 12 drill, defect 2: a stamped `updated` is a *date* a person
    // reads, not a debugging timestamp. The mutation this assertion exists
    // to kill is reverting `ops::knowledge::write`'s `to_rfc3339_opts` back
    // to bare `to_rfc3339`, which reintroduces microseconds (a `.` before
    // the offset) that this crate's own drill found on disk.
    assert!(
        !updated.contains('.'),
        "updated must be second resolution, not sub-second: {updated}"
    );

    let shown = handler
        .handle_query(qry(
            "knowledge.show",
            serde_json::json!({ "name": "my-note" }),
        ))
        .expect("knowledge.show succeeds");
    assert_eq!(shown.result["title"], "My Note");
    assert_eq!(shown.result["body"], "the body text");
    assert_eq!(shown.result["updated"], updated);

    // The daemon stamps `updated`; the caller never supplies it (decision
    // 13) — there is no field in the payload for it at all, so nothing to
    // assert was ignored beyond the fact that it is present and valid.
    assert_eq!(durable_write_count(&fixture, "knowledge", "my-note"), 1);
}

#[test]
fn writing_an_existing_name_without_update_is_refused_and_names_the_flag() {
    let fixture = common::build(&[]);
    let handler = new_handler(&fixture);

    handler
        .handle_command(cmd(
            "knowledge.write",
            write_payload("dup", "First", "first body", false),
        ))
        .expect("first write succeeds");

    let err = handler
        .handle_command(cmd(
            "knowledge.write",
            write_payload("dup", "Second", "second body", false),
        ))
        .unwrap_err();
    assert_eq!(err.code, "conflict.note_already_exists");
    assert!(
        err.message.contains("--update"),
        "the refusal must name the flag that would allow the overwrite: {}",
        err.message
    );

    // The refused write must not have recorded a second provenance row.
    assert_eq!(durable_write_count(&fixture, "knowledge", "dup"), 1);
}

#[test]
fn update_flag_allows_overwrite() {
    let fixture = common::build(&[]);
    let handler = new_handler(&fixture);

    handler
        .handle_command(cmd(
            "knowledge.write",
            write_payload("dup", "First", "first body", false),
        ))
        .expect("first write succeeds");
    handler
        .handle_command(cmd(
            "knowledge.write",
            write_payload("dup", "Second", "second body", true),
        ))
        .expect("update write succeeds");

    let shown = handler
        .handle_query(qry("knowledge.show", serde_json::json!({ "name": "dup" })))
        .expect("knowledge.show succeeds");
    assert_eq!(shown.result["title"], "Second");
    assert_eq!(shown.result["body"], "second body");

    // Both writes are logged — this table is a log of writes, not an index
    // of current notes (`schema::V7_SCHEMA`'s own doc comment on `name`).
    assert_eq!(durable_write_count(&fixture, "knowledge", "dup"), 2);
}

#[test]
fn an_empty_body_is_refused() {
    let fixture = common::build(&[]);
    let handler = new_handler(&fixture);

    let err = handler
        .handle_command(cmd(
            "knowledge.write",
            write_payload("empty", "Title", "   \n  ", false),
        ))
        .unwrap_err();
    assert_eq!(err.code, "validation.empty_body");
    assert_eq!(durable_write_count(&fixture, "knowledge", "empty"), 0);
}

/// Mutation 8's own target: an unreadable file must never silently vanish
/// from `knowledge.list`'s own output, and a dangling `[[link]]` is a gap,
/// not an error.
#[test]
fn list_reports_notes_backlinks_unresolved_links_and_unreadable_files() {
    let fixture = common::build(&[]);
    let handler = new_handler(&fixture);

    handler
        .handle_command(cmd(
            "knowledge.write",
            write_payload("alpha-note", "Alpha", "the target note", false),
        ))
        .expect("write alpha-note");
    handler
        .handle_command(cmd(
            "knowledge.write",
            write_payload(
                "beta-note",
                "Beta",
                "links to [[alpha-note]] and to [[missing-note]]",
                false,
            ),
        ))
        .expect("write beta-note");

    // A file this crate never wrote: a valid note name, unreadable content.
    let knowledge_dir = fixture.instance_root().join(".factory").join("knowledge");
    std::fs::write(knowledge_dir.join("broken.md"), "not frontmatter at all\n")
        .expect("write broken.md");

    let listed = handler
        .handle_query(qry("knowledge.list", serde_json::json!({})))
        .expect("knowledge.list succeeds");

    let notes = listed.result["notes"]
        .as_array()
        .expect("notes is an array");
    assert_eq!(
        notes.len(),
        2,
        "the broken file must not count as a note: {notes:?}"
    );

    let alpha = notes
        .iter()
        .find(|n| n["name"] == "alpha-note")
        .expect("alpha-note is listed");
    let backlinks: Vec<&str> = alpha["backlinks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(backlinks, vec!["beta-note"]);

    let unresolved = listed.result["unresolved"]
        .as_array()
        .expect("unresolved is an array");
    assert_eq!(unresolved.len(), 1);
    assert_eq!(unresolved[0]["from"], "beta-note");
    assert_eq!(unresolved[0]["target"], "missing-note");

    let unreadable = listed.result["unreadable"]
        .as_array()
        .expect("unreadable is an array");
    assert_eq!(
        unreadable.len(),
        1,
        "broken.md must be reported: {unreadable:?}"
    );
    assert_eq!(unreadable[0]["filename"], "broken.md");
}

#[test]
fn listing_before_any_note_is_written_is_an_empty_index_not_an_error() {
    let fixture = common::build(&[]);
    let handler = new_handler(&fixture);

    let listed = handler
        .handle_query(qry("knowledge.list", serde_json::json!({})))
        .expect("knowledge.list succeeds even with no knowledge/ directory yet");
    assert_eq!(listed.result["notes"].as_array().unwrap().len(), 0);
    assert_eq!(listed.result["unresolved"].as_array().unwrap().len(), 0);
    assert_eq!(listed.result["unreadable"].as_array().unwrap().len(), 0);
}

/// Mutation 5's own target: decision 3's ordering. A rename that fails after
/// the `durable_writes` row was appended, inside the still-open transaction,
/// must leave that row rolled back — never durable. Forced here by
/// pre-creating a *directory* at the note's own final path: renaming a file
/// onto an existing non-empty directory fails at the OS level, deterministically,
/// without touching this module's own code.
#[test]
fn a_failed_rename_leaves_no_durable_writes_row() {
    let fixture = common::build(&[]);
    let handler = new_handler(&fixture);

    let knowledge_dir = fixture.instance_root().join(".factory").join("knowledge");
    std::fs::create_dir_all(knowledge_dir.join("blocked.md"))
        .expect("pre-create a directory at the note's own final path");

    // `update: true` skips `stage`'s own pre-existence check (which would
    // otherwise refuse before ever reaching the rename) — the directory at
    // `blocked.md` is exactly as much "already exists" as a real note would
    // be, so bypassing that check is what lets the rename itself run and
    // fail.
    let err = handler
        .handle_command(cmd(
            "knowledge.write",
            write_payload("blocked", "Blocked", "body text", true),
        ))
        .unwrap_err();
    assert_eq!(err.code, "internal.io_error", "{err:?}");

    assert_eq!(
        durable_write_count(&fixture, "knowledge", "blocked"),
        0,
        "a failed rename must leave no durable_writes row behind"
    );
}

#[test]
fn durable_writes_path_is_relative_to_the_instance_root() {
    let fixture = common::build(&[]);
    let handler = new_handler(&fixture);

    handler
        .handle_command(cmd(
            "knowledge.write",
            write_payload("relpath", "Title", "body", false),
        ))
        .expect("write succeeds");

    let store = factory_store::Store::open(fixture.instance_root()).expect("open store");
    let path: String = store
        .connection()
        .query_row(
            "SELECT path FROM durable_writes WHERE kind = 'knowledge' AND name = 'relpath'",
            [],
            |row| row.get(0),
        )
        .expect("row exists");
    assert!(
        !path.starts_with('/'),
        "durable_writes.path must be relative to the instance root, not absolute: {path}"
    );
    assert!(path.contains("knowledge"), "{path}");
}
