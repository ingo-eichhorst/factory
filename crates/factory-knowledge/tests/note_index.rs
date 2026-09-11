//! The derived index: unresolved links (the single most important
//! behaviour of this module), backlinks, determinism independent of
//! directory-read order, and reproducibility after the directory is
//! deleted and the same notes are restored.

use factory_knowledge::note::{self, Frontmatter, Note, NoteName};
use tempfile::tempdir;

fn name(s: &str) -> NoteName {
    NoteName::parse(s).expect("valid name")
}

fn write_note(dir: &std::path::Path, name_str: &str, title: &str, body: &str) {
    let n = Note::new(
        name(name_str),
        Frontmatter {
            title: title.to_string(),
            status: "draft".to_string(),
            updated: "2026-09-11".to_string(),
            sources: vec!["notes/src.txt".to_string()],
        },
        body.to_string(),
    )
    .expect("valid note");
    note::stage(dir, &n, false).unwrap().commit().unwrap();
}

#[test]
fn a_link_to_a_note_that_does_not_exist_is_unresolved_not_an_error() {
    let dir = tempdir().unwrap();
    write_note(dir.path(), "a-note", "A", "Links to [[missing-note]].");

    let index = note::index(dir.path()).expect("index never errors on a dangling link");

    assert_eq!(
        index.unresolved,
        vec![(name("a-note"), "missing-note".to_string())]
    );
    assert_eq!(index.notes.len(), 1);
    assert!(index.notes[0].backlinks.is_empty());
}

#[test]
fn a_link_to_a_real_note_produces_a_backlink_and_no_unresolved_entry() {
    let dir = tempdir().unwrap();
    write_note(dir.path(), "a-note", "A", "Links to [[b-note]].");
    write_note(dir.path(), "b-note", "B", "No links here.");

    let index = note::index(dir.path()).unwrap();
    assert!(index.unresolved.is_empty());

    let b_entry = index
        .notes
        .iter()
        .find(|e| e.name == name("b-note"))
        .unwrap();
    assert_eq!(b_entry.backlinks, vec![name("a-note")]);

    let a_entry = index
        .notes
        .iter()
        .find(|e| e.name == name("a-note"))
        .unwrap();
    assert!(a_entry.backlinks.is_empty());
    assert_eq!(a_entry.links, vec!["b-note".to_string()]);
}

#[test]
fn index_is_sorted_by_name_regardless_of_write_order() {
    let dir = tempdir().unwrap();
    // Written in an order that is neither alphabetical nor reversed, so a
    // missing sort would not accidentally look sorted anyway.
    write_note(dir.path(), "c-note", "C", "No links.");
    write_note(dir.path(), "a-note", "A", "No links.");
    write_note(dir.path(), "b-note", "B", "No links.");

    let index = note::index(dir.path()).unwrap();
    let names: Vec<&str> = index.notes.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["a-note", "b-note", "c-note"]);
}

#[test]
fn backlinks_and_unresolved_are_sorted_deterministically() {
    let dir = tempdir().unwrap();
    // Multiple notes link to the same target, and multiple links are
    // unresolved, written in an order that does not match the expected
    // sorted output.
    write_note(
        dir.path(),
        "c-note",
        "C",
        "Links to [[target]] and [[zeta-missing]].",
    );
    write_note(
        dir.path(),
        "a-note",
        "A",
        "Links to [[target]] and [[alpha-missing]].",
    );
    write_note(dir.path(), "target", "Target", "No links.");

    let index = note::index(dir.path()).unwrap();

    let target_entry = index
        .notes
        .iter()
        .find(|e| e.name == name("target"))
        .unwrap();
    assert_eq!(target_entry.backlinks, vec![name("a-note"), name("c-note")]);

    assert_eq!(
        index.unresolved,
        vec![
            (name("a-note"), "alpha-missing".to_string()),
            (name("c-note"), "zeta-missing".to_string()),
        ]
    );
}

#[test]
fn index_run_twice_on_unchanged_notes_is_identical() {
    let dir = tempdir().unwrap();
    write_note(
        dir.path(),
        "a-note",
        "A",
        "Links to [[b-note]] and [[missing]].",
    );
    write_note(dir.path(), "b-note", "B", "No links.");

    let first = note::index(dir.path()).unwrap();
    let second = note::index(dir.path()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn deleting_and_restoring_the_same_notes_reproduces_the_same_index() {
    let dir = tempdir().unwrap();
    write_note(
        dir.path(),
        "a-note",
        "A",
        "Links to [[b-note]] and [[missing]].",
    );
    write_note(dir.path(), "b-note", "B", "No links.");

    let before = note::index(dir.path()).unwrap();

    std::fs::remove_dir_all(dir.path()).unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();

    write_note(
        dir.path(),
        "a-note",
        "A",
        "Links to [[b-note]] and [[missing]].",
    );
    write_note(dir.path(), "b-note", "B", "No links.");

    let after = note::index(dir.path()).unwrap();
    assert_eq!(before, after);
}

#[test]
fn a_stray_non_conforming_file_is_skipped_not_treated_as_a_corrupt_note() {
    let dir = tempdir().unwrap();
    write_note(dir.path(), "a-note", "A", "No links.");
    std::fs::write(dir.path().join(".gitkeep"), b"").unwrap();
    std::fs::write(dir.path().join("README.md"), b"not a note at all").unwrap();

    let index = note::index(dir.path()).unwrap();
    assert_eq!(index.notes.len(), 1);
    assert_eq!(index.notes[0].name, name("a-note"));
}

#[test]
fn a_md_file_with_a_valid_note_name_but_corrupt_content_is_reported_not_fatal() {
    // Unlike a stray non-conforming filename (skipped, above), a file this
    // crate would have written a note under — a valid `NoteName` stem with
    // a `.md` extension — holding text this crate did not write is a real
    // problem, but not one that should take the whole index down: one bad
    // file must not hide the other ninety-nine notes in `dir`.
    let dir = tempdir().unwrap();
    write_note(dir.path(), "a-note", "A", "No links.");
    std::fs::write(
        dir.path().join("corrupt-note.md"),
        b"not frontmatter at all",
    )
    .unwrap();

    let index = note::index(dir.path()).expect("one bad file must not abort the whole index");
    assert_eq!(index.notes.len(), 1);
    assert_eq!(index.notes[0].name, name("a-note"));
    assert_eq!(index.unreadable.len(), 1);
    assert_eq!(index.unreadable[0].0, "corrupt-note.md");
    assert!(!index.unreadable[0].1.is_empty());
}

#[test]
fn unreadable_entries_are_sorted_deterministically() {
    let dir = tempdir().unwrap();
    write_note(dir.path(), "a-note", "A", "No links.");
    // Written in an order that does not match the expected sorted output.
    std::fs::write(dir.path().join("zeta-bad.md"), b"garbage").unwrap();
    std::fs::write(dir.path().join("alpha-bad.md"), b"garbage").unwrap();

    let index = note::index(dir.path()).unwrap();
    let filenames: Vec<&str> = index.unreadable.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(filenames, vec!["alpha-bad.md", "zeta-bad.md"]);
}

#[test]
fn a_link_to_an_unreadable_note_is_unresolved_not_a_backlink() {
    // The broken note contributed nothing to `existing`, so a link to it
    // is reported the same way a link to a name with no file at all is —
    // the linking note did nothing wrong; the problem is recorded against
    // the broken file itself, in `unreadable`.
    let dir = tempdir().unwrap();
    write_note(dir.path(), "a-note", "A", "Links to [[broken-note]].");
    std::fs::write(dir.path().join("broken-note.md"), b"garbage").unwrap();

    let index = note::index(dir.path()).unwrap();
    assert_eq!(
        index.unresolved,
        vec![(name("a-note"), "broken-note".to_string())]
    );
    assert_eq!(index.unreadable.len(), 1);
}

#[test]
fn frontmatter_fields_in_a_different_order_still_index_correctly() {
    // The most ordinary edit a person makes to a note: reordering two
    // lines. `write_note`/`Note::render` always write the canonical
    // order, so this writes the raw text by hand instead.
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join("reordered.md"),
        "---\nsources:\n  - notes/src.txt\nupdated: 2026-09-11\ntitle: Reordered\nstatus: draft\n---\n\nNo links.",
    )
    .unwrap();

    let index = note::index(dir.path()).expect("reordered frontmatter must still parse");
    assert_eq!(index.notes.len(), 1);
    assert_eq!(index.notes[0].title, "Reordered");
    assert!(index.unreadable.is_empty());
}

#[test]
fn an_orphaned_temp_file_from_an_uncommitted_stage_is_invisible_to_index() {
    let dir = tempdir().unwrap();
    write_note(dir.path(), "a-note", "A", "No links.");

    let extra = Note::new(
        name("b-note"),
        Frontmatter {
            title: "B".to_string(),
            status: "draft".to_string(),
            updated: "2026-09-11".to_string(),
            sources: vec!["notes/src.txt".to_string()],
        },
        "No links.".to_string(),
    )
    .unwrap();
    // Leaked deliberately: forget the `Staged` without dropping it or
    // committing it, so the temp file survives (mem::forget skips `Drop`,
    // simulating a process killed before either happened).
    let staged = note::stage(dir.path(), &extra, false).unwrap();
    std::mem::forget(staged);

    let index = note::index(dir.path()).unwrap();
    assert_eq!(index.notes.len(), 1);
    assert_eq!(index.notes[0].name, name("a-note"));
}
