//! The staged write: `stage`, `commit`, `Drop`, `read`, and the "never
//! truncated" guarantee ADR 0022 decision 3 asks for.

use std::fs;

use factory_knowledge::note::{self, Frontmatter, Note, NoteError, NoteName};
use tempfile::tempdir;

fn name(s: &str) -> NoteName {
    NoteName::parse(s).expect("valid name")
}

fn note(title: &str, body: &str) -> Note {
    Note::new(
        name("weekly-sync"),
        Frontmatter {
            title: title.to_string(),
            status: "draft".to_string(),
            updated: "2026-09-11".to_string(),
            sources: vec!["notes/a.txt".to_string()],
        },
        body.to_string(),
    )
    .expect("valid note")
}

/// Every entry in `dir`, by filename, sorted — used to check that staging
/// or dropping left exactly the files a test expects and nothing else,
/// rather than checking one path in isolation and missing a stray leftover.
fn dir_listing(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn stage_then_commit_makes_the_note_appear_under_its_real_name() {
    let dir = tempdir().unwrap();
    let n = note("Weekly sync", "Hello.");

    let staged = note::stage(dir.path(), &n, false).unwrap();
    let expected_path = staged.path().to_path_buf();
    assert_eq!(expected_path, dir.path().join("weekly-sync.md"));

    // Nothing is visible under the real name until commit.
    assert!(!expected_path.exists());

    let committed_path = staged.commit().unwrap();
    assert_eq!(committed_path, expected_path);
    assert_eq!(fs::read_to_string(&committed_path).unwrap(), n.render());
}

#[test]
fn read_returns_what_was_staged_and_committed() {
    let dir = tempdir().unwrap();
    let n = note("Weekly sync", "Hello.");
    note::stage(dir.path(), &n, false)
        .unwrap()
        .commit()
        .unwrap();

    let read_back = note::read(dir.path(), &name("weekly-sync")).unwrap();
    assert_eq!(read_back, n);
}

#[test]
fn read_of_a_missing_note_is_not_found() {
    let dir = tempdir().unwrap();
    let err = note::read(dir.path(), &name("does-not-exist")).unwrap_err();
    assert!(matches!(err, NoteError::NotFound(_)));
}

#[test]
fn read_of_a_file_with_corrupt_content_is_malformed_not_silently_accepted() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("weekly-sync.md"), b"not frontmatter at all").unwrap();
    let err = note::read(dir.path(), &name("weekly-sync")).unwrap_err();
    assert!(matches!(err, NoteError::MalformedFrontmatter(_)));
}

#[test]
fn stage_without_update_refuses_an_existing_note() {
    let dir = tempdir().unwrap();
    let n = note("Weekly sync", "Hello.");
    note::stage(dir.path(), &n, false)
        .unwrap()
        .commit()
        .unwrap();

    let again = note("Weekly sync v2", "Hello again.");
    let err = note::stage(dir.path(), &again, false).unwrap_err();
    assert!(matches!(err, NoteError::AlreadyExists(ref s) if s == "weekly-sync"));

    // Refused before anything touched the real file.
    let on_disk = fs::read_to_string(dir.path().join("weekly-sync.md")).unwrap();
    assert_eq!(on_disk, n.render());
}

#[test]
fn stage_with_update_overwrites_an_existing_note() {
    let dir = tempdir().unwrap();
    let original = note("Weekly sync", "Hello.");
    note::stage(dir.path(), &original, false)
        .unwrap()
        .commit()
        .unwrap();

    let updated = note(
        "Weekly sync v2",
        "Hello again, at greater length than before.",
    );
    note::stage(dir.path(), &updated, true)
        .unwrap()
        .commit()
        .unwrap();

    let read_back = note::read(dir.path(), &name("weekly-sync")).unwrap();
    assert_eq!(read_back, updated);
}

#[test]
fn a_dropped_staged_leaves_no_temporary_file_behind() {
    let dir = tempdir().unwrap();
    let n = note("Weekly sync", "Hello.");

    {
        let _staged = note::stage(dir.path(), &n, false).unwrap();
        // Some temp file exists in `dir` right now — staging wrote it.
        assert_eq!(dir_listing(dir.path()).len(), 1);
    }
    // Dropped without `commit`: the directory must be empty again. Checking
    // `staged.path().exists()` would not prove this — that path is the
    // *final* name, which was never created either way; only enumerating
    // the directory catches a leaked temp file.
    assert!(dir_listing(dir.path()).is_empty());
}

#[test]
fn dropping_an_uncommitted_stage_leaves_an_existing_note_untouched() {
    let dir = tempdir().unwrap();
    let original = note("Weekly sync", "Hello.");
    note::stage(dir.path(), &original, false)
        .unwrap()
        .commit()
        .unwrap();

    let updated = note(
        "Weekly sync v2",
        "Hello again, at much greater length than the original body.",
    );
    {
        let staged = note::stage(dir.path(), &updated, true).unwrap();
        // Staged but never committed — simulates a crash between staging
        // and the store transaction that was going to commit it.
        drop(staged);
    }

    // The real file must still hold the complete original bytes, never a
    // mix of old and new and never truncated, and the directory holds
    // exactly the one real file — no orphaned temp file beside it.
    let on_disk = fs::read_to_string(dir.path().join("weekly-sync.md")).unwrap();
    assert_eq!(on_disk, original.render());
    assert_eq!(dir_listing(dir.path()), vec!["weekly-sync.md".to_string()]);
}

#[test]
fn staging_an_update_does_not_touch_the_real_file_before_commit() {
    // The core of "never truncated": the destination is only ever replaced
    // by a single atomic `rename`, so there is no instant, success or
    // failure, at which a reader could see a partial file.
    let dir = tempdir().unwrap();
    let original = note("Weekly sync", "Original body.");
    note::stage(dir.path(), &original, false)
        .unwrap()
        .commit()
        .unwrap();

    let updated = note(
        "Weekly sync v2",
        "Replacement body, deliberately longer than the original.",
    );
    let staged = note::stage(dir.path(), &updated, true).unwrap();

    // Before commit: real file is still exactly the original.
    let before = fs::read_to_string(dir.path().join("weekly-sync.md")).unwrap();
    assert_eq!(before, original.render());

    staged.commit().unwrap();

    // After commit: real file is exactly the update, not a splice of both.
    let after = fs::read_to_string(dir.path().join("weekly-sync.md")).unwrap();
    assert_eq!(after, updated.render());
}
