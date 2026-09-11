//! Scope-local memory: staging, chronological listing, the empty-entry
//! refusal, "never rewritten", and the scope-escape guard.

use factory_knowledge::memory::{self, MemoryEntry};
use factory_knowledge::note::NoteError;
use tempfile::tempdir;

/// A deterministic, distinct UUID built by hand from a seed — this
/// workspace pins `uuid = "1"` with no `v4` feature (see
/// `factory-delegation`'s tests for the same convention), so nothing here
/// calls `Uuid::new_v4()`.
fn uuid(seed: u32) -> uuid::Uuid {
    uuid::Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

#[test]
fn stage_then_commit_makes_the_entry_readable() {
    let root = tempdir().unwrap();
    // `+00:00`, matching what `ops::memory::add` actually stamps
    // (`chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, false)`) —
    // the round trip through the compact filename always reconstructs this
    // notation (see `expand_timestamp`'s own doc comment), so this is the
    // one shape a caller gets back byte for byte.
    let staged = memory::stage_entry(
        root.path(),
        "team-alpha",
        "first entry",
        uuid(1),
        "2026-09-11T10:00:00+00:00",
    )
    .unwrap();
    staged.commit().unwrap();

    let entries = memory::list_entries(root.path(), "team-alpha").unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, uuid(1));
    assert_eq!(entries[0].text, "first entry");
    assert_eq!(entries[0].created_at, "2026-09-11T10:00:00+00:00");
}

// ---------------------------------------------------------------------
// Station 12 drill, defect 1: the filename must survive ordinary tooling.
// ---------------------------------------------------------------------

/// The exact shape observed on disk during the drill: microsecond
/// precision, a `+00:00` offset — two colons in the time and one in the
/// offset, none of which may reach the filename.
#[test]
fn the_filename_contains_no_character_ordinary_tooling_treats_specially() {
    let root = tempdir().unwrap();
    let staged = memory::stage_entry(
        root.path(),
        "team-alpha",
        "entry",
        uuid(1),
        "2026-09-11T08:55:05.014184+00:00",
    )
    .unwrap();

    let filename = staged
        .path()
        .file_name()
        .and_then(|n| n.to_str())
        .expect("staged path has a filename")
        .to_string();

    // `rsync` treats a colon specially (a remote host prefix); Windows
    // refuses several of these outright; Finder renders `:` as `/`. None of
    // them may appear, in a filename this crate produces itself.
    const RESERVED: &[char] = &[':', '<', '>', '"', '/', '\\', '|', '?', '*'];
    for reserved in RESERVED {
        assert!(
            !filename.contains(*reserved),
            "filename {filename:?} must not contain {reserved:?}"
        );
    }
}

/// The mutation this test exists to kill: putting the colons back. A
/// canonical UTC, second-resolution `created_at` (exactly what
/// `ops::memory::add` stamps after the defect 2 fix) must compact to
/// exactly the ISO 8601 basic form ADR 0022's own report names.
#[test]
fn a_canonical_utc_timestamp_compacts_to_the_exact_basic_form() {
    let root = tempdir().unwrap();
    let staged = memory::stage_entry(
        root.path(),
        "team-alpha",
        "entry",
        uuid(1),
        "2026-09-11T08:55:05+00:00",
    )
    .unwrap();

    let filename = staged
        .path()
        .file_name()
        .and_then(|n| n.to_str())
        .expect("staged path has a filename")
        .to_string();

    assert_eq!(
        filename,
        format!("20260911T085505Z--{}.md", uuid(1)),
        "the filename must be the compact ISO 8601 basic stamp, not the raw RFC 3339 one"
    );
}

// ---------------------------------------------------------------------
// Station 12 drill, defect 2: second-resolution timestamps can collide.
// ---------------------------------------------------------------------

/// Two entries stamped in the same second must still land in two distinct
/// files (the `id` suffix, not the timestamp, is what makes a filename
/// unique) and `list_entries` must still return both, in a total,
/// deterministic order — the exact thing reducing `created_at` to second
/// resolution makes far more likely to actually happen.
#[test]
fn same_second_entries_stay_distinct_and_totally_ordered() {
    let root = tempdir().unwrap();
    // `uuid(1) < uuid(2)` under `Uuid`'s own byte-wise `Ord` — this pins
    // which order `list_entries` must return them in, rather than merely
    // asserting both are present.
    memory::stage_entry(
        root.path(),
        "team-alpha",
        "second by id",
        uuid(2),
        "2026-09-11T08:55:05+00:00",
    )
    .unwrap()
    .commit()
    .unwrap();
    memory::stage_entry(
        root.path(),
        "team-alpha",
        "first by id",
        uuid(1),
        "2026-09-11T08:55:05+00:00",
    )
    .unwrap()
    .commit()
    .unwrap();

    let scope_dir = root.path().join("team-alpha");
    let file_count = std::fs::read_dir(&scope_dir).unwrap().count();
    assert_eq!(
        file_count, 2,
        "same-second entries must land in two distinct files, not overwrite each other"
    );

    let entries = memory::list_entries(root.path(), "team-alpha").unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
        vec!["first by id", "second by id"],
        "a tied created_at must still fall back to a total order, here by id"
    );
}

#[test]
fn listing_an_unwritten_scope_is_an_empty_list_not_an_error() {
    let root = tempdir().unwrap();
    let entries = memory::list_entries(root.path(), "never-used").unwrap();
    assert!(entries.is_empty());
}

#[test]
fn scopes_are_isolated_from_each_other() {
    let root = tempdir().unwrap();
    memory::stage_entry(
        root.path(),
        "team-alpha",
        "alpha entry",
        uuid(1),
        "2026-09-11T10:00:00Z",
    )
    .unwrap()
    .commit()
    .unwrap();
    memory::stage_entry(
        root.path(),
        "team-beta",
        "beta entry",
        uuid(2),
        "2026-09-11T10:00:00Z",
    )
    .unwrap()
    .commit()
    .unwrap();

    let alpha = memory::list_entries(root.path(), "team-alpha").unwrap();
    assert_eq!(alpha.len(), 1);
    assert_eq!(alpha[0].text, "alpha entry");
}

#[test]
fn list_entries_is_sorted_chronologically_regardless_of_write_order() {
    let root = tempdir().unwrap();
    // Written out of chronological order on purpose.
    memory::stage_entry(
        root.path(),
        "team-alpha",
        "second",
        uuid(2),
        "2026-09-11T12:00:00Z",
    )
    .unwrap()
    .commit()
    .unwrap();
    memory::stage_entry(
        root.path(),
        "team-alpha",
        "first",
        uuid(1),
        "2026-09-11T09:00:00Z",
    )
    .unwrap()
    .commit()
    .unwrap();
    memory::stage_entry(
        root.path(),
        "team-alpha",
        "third",
        uuid(3),
        "2026-09-11T15:00:00Z",
    )
    .unwrap()
    .commit()
    .unwrap();

    let entries = memory::list_entries(root.path(), "team-alpha").unwrap();
    let texts: Vec<&str> = entries
        .iter()
        .map(|e: &MemoryEntry| e.text.as_str())
        .collect();
    assert_eq!(texts, vec!["first", "second", "third"]);
}

#[test]
fn refuses_an_empty_entry() {
    let root = tempdir().unwrap();
    let err = memory::stage_entry(
        root.path(),
        "team-alpha",
        "",
        uuid(1),
        "2026-09-11T10:00:00Z",
    )
    .unwrap_err();
    assert!(matches!(err, NoteError::EmptyEntry));
}

#[test]
fn refuses_a_whitespace_only_entry() {
    let root = tempdir().unwrap();
    let err = memory::stage_entry(
        root.path(),
        "team-alpha",
        "   \n\t",
        uuid(1),
        "2026-09-11T10:00:00Z",
    )
    .unwrap_err();
    assert!(matches!(err, NoteError::EmptyEntry));
}

#[test]
fn an_existing_entry_is_never_rewritten() {
    let root = tempdir().unwrap();
    let id = uuid(1);
    let staged = memory::stage_entry(
        root.path(),
        "team-alpha",
        "original text",
        id,
        "2026-09-11T10:00:00Z",
    )
    .unwrap();
    let path = staged.path().to_path_buf();
    staged.commit().unwrap();
    let original_on_disk = std::fs::read_to_string(&path).unwrap();
    assert_eq!(original_on_disk, "original text");

    // Same id, same timestamp — computes to the exact same filename. This
    // must be refused, not silently applied, or a retry could rewrite an
    // entry that was already committed.
    let err = memory::stage_entry(
        root.path(),
        "team-alpha",
        "replacement text",
        id,
        "2026-09-11T10:00:00Z",
    )
    .unwrap_err();
    assert!(matches!(err, NoteError::DuplicateEntry(_)));

    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after, "original text");
}

// ---------------------------------------------------------------------
// Scope escape
// ---------------------------------------------------------------------

#[test]
fn refuses_a_scope_of_exactly_dot_dot() {
    // The discriminating case: `contains('/')` alone does not catch this
    // one, so it is the fixture that actually exercises the `..` check
    // rather than being caught by the slash check instead.
    let root = tempdir().unwrap();
    let err = memory::stage_entry(root.path(), "..", "entry", uuid(1), "2026-09-11T10:00:00Z")
        .unwrap_err();
    assert!(matches!(err, NoteError::InvalidScope(ref s) if s == ".."));
}

#[test]
fn refuses_a_scope_containing_dot_dot_and_a_slash() {
    let root = tempdir().unwrap();
    let err = memory::stage_entry(
        root.path(),
        "../escaped",
        "entry",
        uuid(1),
        "2026-09-11T10:00:00Z",
    )
    .unwrap_err();
    assert!(matches!(err, NoteError::InvalidScope(_)));
}

#[test]
fn refuses_a_scope_containing_a_slash() {
    let root = tempdir().unwrap();
    let err = memory::stage_entry(
        root.path(),
        "team/alpha",
        "entry",
        uuid(1),
        "2026-09-11T10:00:00Z",
    )
    .unwrap_err();
    assert!(matches!(err, NoteError::InvalidScope(_)));
}

#[test]
fn refuses_an_absolute_path_scope() {
    let root = tempdir().unwrap();
    let err = memory::stage_entry(
        root.path(),
        "/etc/passwd",
        "entry",
        uuid(1),
        "2026-09-11T10:00:00Z",
    )
    .unwrap_err();
    assert!(matches!(err, NoteError::InvalidScope(_)));
}

#[test]
fn refuses_an_empty_scope() {
    let root = tempdir().unwrap();
    let err =
        memory::stage_entry(root.path(), "", "entry", uuid(1), "2026-09-11T10:00:00Z").unwrap_err();
    assert!(matches!(err, NoteError::InvalidScope(_)));
}

#[test]
fn a_scope_escape_attempt_writes_nothing_outside_memory_root() {
    let root = tempdir().unwrap();
    let outside_marker = root
        .path()
        .parent()
        .unwrap()
        .join("factory-knowledge-escape-test-marker");
    let _ = std::fs::remove_file(&outside_marker);

    let attempt = memory::stage_entry(
        root.path(),
        "../factory-knowledge-escape-test-marker",
        "entry",
        uuid(1),
        "2026-09-11T10:00:00Z",
    );
    assert!(attempt.is_err());
    assert!(!outside_marker.exists());
}

#[test]
fn accepts_an_ordinary_scope_name() {
    let root = tempdir().unwrap();
    assert!(
        memory::stage_entry(
            root.path(),
            "team-alpha",
            "entry",
            uuid(1),
            "2026-09-11T10:00:00Z"
        )
        .is_ok()
    );
}
