//! Name rules, frontmatter completeness, the secret-source check, the
//! empty-body check, and the render/parse round trip — everything in ADR
//! 0022 that does not touch a filesystem.
//!
//! Every fixture below is otherwise fully valid, changing exactly one
//! thing, so that the error variant asserted is the one and only variant
//! the fixture can trigger — the acceptance standard's own example of a
//! test that "still passes with the behaviour removed" is a fixture that
//! violates two rules at once and only proves the first one to fire.

use factory_knowledge::note::{Frontmatter, Note, NoteError, NoteName};

fn name(s: &str) -> NoteName {
    NoteName::parse(s).expect("valid name")
}

fn valid_frontmatter() -> Frontmatter {
    Frontmatter {
        title: "Weekly sync".to_string(),
        status: "draft".to_string(),
        updated: "2026-09-11".to_string(),
        sources: vec!["notes/2026-09-11.txt".to_string()],
    }
}

// ---------------------------------------------------------------------
// Name rules (decision 6)
// ---------------------------------------------------------------------

#[test]
fn accepts_a_plain_lowercase_hyphenated_name() {
    assert!(NoteName::parse("weekly-sync-2026").is_ok());
}

#[test]
fn accepts_a_single_character_name() {
    assert!(NoteName::parse("a").is_ok());
}

#[test]
fn accepts_a_sixty_four_character_name() {
    let n = "a".repeat(64);
    assert!(NoteName::parse(&n).is_ok());
}

#[test]
fn refuses_a_name_over_sixty_four_characters() {
    let n = "a".repeat(65);
    assert!(matches!(
        NoteName::parse(&n),
        Err(NoteError::InvalidName(_))
    ));
}

#[test]
fn refuses_an_empty_name() {
    assert!(matches!(
        NoteName::parse(""),
        Err(NoteError::InvalidName(_))
    ));
}

#[test]
fn refuses_uppercase() {
    assert!(matches!(
        NoteName::parse("Weekly-Sync"),
        Err(NoteError::InvalidName(_))
    ));
}

#[test]
fn refuses_a_leading_hyphen() {
    assert!(matches!(
        NoteName::parse("-weekly-sync"),
        Err(NoteError::InvalidName(_))
    ));
}

#[test]
fn refuses_a_trailing_hyphen() {
    assert!(matches!(
        NoteName::parse("weekly-sync-"),
        Err(NoteError::InvalidName(_))
    ));
}

#[test]
fn refuses_consecutive_hyphens() {
    assert!(matches!(
        NoteName::parse("weekly--sync"),
        Err(NoteError::InvalidName(_))
    ));
}

#[test]
fn refuses_a_character_outside_the_set() {
    assert!(matches!(
        NoteName::parse("weekly_sync"),
        Err(NoteError::InvalidName(_))
    ));
    assert!(matches!(
        NoteName::parse("weekly sync"),
        Err(NoteError::InvalidName(_))
    ));
    assert!(matches!(
        NoteName::parse("weekly.sync"),
        Err(NoteError::InvalidName(_))
    ));
}

// ---------------------------------------------------------------------
// Frontmatter completeness (decision 2)
// ---------------------------------------------------------------------

#[test]
fn refuses_missing_title() {
    let mut fm = valid_frontmatter();
    fm.title = String::new();
    let err = Note::new(name("n"), fm, "some body text".to_string()).unwrap_err();
    assert!(matches!(err, NoteError::MissingField("title")));
}

#[test]
fn refuses_missing_status() {
    let mut fm = valid_frontmatter();
    fm.status = String::new();
    let err = Note::new(name("n"), fm, "some body text".to_string()).unwrap_err();
    assert!(matches!(err, NoteError::MissingField("status")));
}

#[test]
fn refuses_missing_updated() {
    let mut fm = valid_frontmatter();
    fm.updated = String::new();
    let err = Note::new(name("n"), fm, "some body text".to_string()).unwrap_err();
    assert!(matches!(err, NoteError::MissingField("updated")));
}

#[test]
fn refuses_zero_sources() {
    let mut fm = valid_frontmatter();
    fm.sources = vec![];
    let err = Note::new(name("n"), fm, "some body text".to_string()).unwrap_err();
    assert!(matches!(err, NoteError::MissingField("sources")));
}

#[test]
fn refuses_a_sources_list_of_nothing_but_blanks() {
    // `sources: [""]` is zero real sources wearing a non-empty `Vec` — the
    // gap `validate`'s doc comment calls out explicitly.
    let mut fm = valid_frontmatter();
    fm.sources = vec!["".to_string(), "   ".to_string()];
    let err = Note::new(name("n"), fm, "some body text".to_string()).unwrap_err();
    assert!(matches!(err, NoteError::MissingField("sources")));
}

#[test]
fn accepts_a_blank_source_alongside_a_real_one() {
    // Not a completeness failure: at least one real source is present.
    let mut fm = valid_frontmatter();
    fm.sources = vec!["".to_string(), "notes/real.txt".to_string()];
    assert!(Note::new(name("n"), fm, "some body text".to_string()).is_ok());
}

// ---------------------------------------------------------------------
// Secret sources (decision 3 / rule 3)
// ---------------------------------------------------------------------

fn note_with_source(source: &str) -> Result<Note, NoteError> {
    let mut fm = valid_frontmatter();
    fm.sources = vec![source.to_string()];
    Note::new(name("n"), fm, "some body text".to_string())
}

#[test]
fn refuses_a_plain_relative_secret_source() {
    assert!(matches!(
        note_with_source("data/secrets/x"),
        Err(NoteError::SecretSource { index: 1 })
    ));
}

#[test]
fn refuses_a_dot_slash_prefixed_secret_source() {
    assert!(matches!(
        note_with_source("./data/secrets/x"),
        Err(NoteError::SecretSource { index: 1 })
    ));
}

#[test]
fn refuses_an_absolutely_rooted_secret_source() {
    assert!(matches!(
        note_with_source("/abs/data/secrets/x"),
        Err(NoteError::SecretSource { index: 1 })
    ));
}

#[test]
fn refuses_the_secrets_directory_itself_with_no_trailing_component() {
    assert!(matches!(
        note_with_source("data/secrets"),
        Err(NoteError::SecretSource { index: 1 })
    ));
}

#[test]
fn does_not_flag_an_unrelated_path_that_merely_contains_the_word_secrets() {
    assert!(note_with_source("docs/data/secrets-similar/x").is_ok());
    assert!(note_with_source("data/public/secrets_of_success.md").is_ok());
}

#[test]
fn secret_source_error_never_quotes_the_path() {
    // Rule 3: the refusal MUST NOT quote the source path's contents.
    let err = note_with_source("data/secrets/api-key").unwrap_err();
    let message = err.to_string();
    assert!(!message.contains("data/secrets/api-key"));
    assert!(!message.contains("api-key"));
}

#[test]
fn secret_source_index_reflects_position_in_the_list() {
    let mut fm = valid_frontmatter();
    fm.sources = vec!["notes/fine.txt".to_string(), "data/secrets/x".to_string()];
    let err = Note::new(name("n"), fm, "some body text".to_string()).unwrap_err();
    assert!(matches!(err, NoteError::SecretSource { index: 2 }));
}

// ---------------------------------------------------------------------
// Source path traversal (`..`)
// ---------------------------------------------------------------------

#[test]
fn refuses_a_source_that_walks_upward_into_the_secrets_directory() {
    // `data/foo/../secrets/key.pem` splits into `[data, foo, .., secrets]`
    // — no adjacent pair is `(data, secrets)`, so an unresolved
    // component-pair check alone would wave this through even though the
    // path is under `data/secrets/` once resolved. The dedicated `..`
    // refusal is what actually closes this gap.
    assert!(matches!(
        note_with_source("data/foo/../secrets/key.pem"),
        Err(NoteError::SourceTraversal { index: 1 })
    ));
}

#[test]
fn refuses_a_source_with_a_parent_component_even_when_unrelated_to_secrets() {
    // The rule is unconditional: any `..` component is refused, not only
    // ones that happen to reach into `data/secrets/`.
    assert!(matches!(
        note_with_source("notes/../outside.txt"),
        Err(NoteError::SourceTraversal { index: 1 })
    ));
}

#[test]
fn refuses_a_leading_parent_component() {
    assert!(matches!(
        note_with_source("../escaped.txt"),
        Err(NoteError::SourceTraversal { index: 1 })
    ));
}

#[test]
fn does_not_flag_a_source_with_two_dots_that_are_not_a_path_component() {
    // `..` must be a whole path component, not merely a substring — a
    // filename like `v1.2..txt` (unusual but not a traversal) must not be
    // caught by a naive substring search.
    assert!(note_with_source("notes/v1.2..txt").is_ok());
}

// ---------------------------------------------------------------------
// Empty body (decision 4)
// ---------------------------------------------------------------------

#[test]
fn refuses_an_empty_body() {
    let err = Note::new(name("n"), valid_frontmatter(), String::new()).unwrap_err();
    assert!(matches!(err, NoteError::EmptyBody));
}

#[test]
fn refuses_a_whitespace_only_body() {
    let err = Note::new(name("n"), valid_frontmatter(), "   \n\t \n".to_string()).unwrap_err();
    assert!(matches!(err, NoteError::EmptyBody));
}

#[test]
fn accepts_a_real_body() {
    assert!(Note::new(name("n"), valid_frontmatter(), "Hello.".to_string()).is_ok());
}

// ---------------------------------------------------------------------
// render / parse round trip
// ---------------------------------------------------------------------

#[test]
fn parse_of_render_reproduces_the_same_note() {
    let mut fm = valid_frontmatter();
    fm.sources = vec!["notes/a.txt".to_string(), "notes/b.txt".to_string()];
    let note = Note::new(
        name("weekly-sync"),
        fm,
        "Line one.\nLine two with a [[link]].".to_string(),
    )
    .expect("valid note");

    let rendered = note.render();
    let parsed = Note::parse(name("weekly-sync"), &rendered).expect("renders back cleanly");

    assert_eq!(note, parsed);
}

#[test]
fn render_parse_round_trip_survives_a_body_that_looks_like_frontmatter() {
    // `parse_frontmatter` locates the body by a fixed line-count offset,
    // never by re-scanning for `---` or `key: value` inside it, so a body
    // that happens to contain lines shaped like the frontmatter it follows
    // must not confuse the reader on the way back in.
    let body = "---\ntitle: not frontmatter\n---\n\nreal text".to_string();
    let note = Note::new(name("n"), valid_frontmatter(), body).expect("valid note");
    let parsed = Note::parse(name("n"), &note.render()).expect("parses");
    assert_eq!(note, parsed);
}

#[test]
fn render_parse_round_trip_preserves_a_body_with_no_trailing_newline() {
    let note = Note::new(
        name("n"),
        valid_frontmatter(),
        "no trailing newline here".to_string(),
    )
    .expect("valid note");
    let parsed = Note::parse(name("n"), &note.render()).expect("parses");
    assert_eq!(note.body, parsed.body);
}

#[test]
fn render_parse_round_trip_preserves_a_body_with_a_trailing_newline() {
    let note = Note::new(
        name("n"),
        valid_frontmatter(),
        "has a trailing newline\n".to_string(),
    )
    .expect("valid note");
    let parsed = Note::parse(name("n"), &note.render()).expect("parses");
    assert_eq!(note.body, parsed.body);
}

#[test]
fn links_are_returned_in_order_of_first_appearance_and_deduplicated() {
    let note = Note::new(
        name("n"),
        valid_frontmatter(),
        "See [[b-note]] and [[a-note]], and again [[b-note]].".to_string(),
    )
    .expect("valid note");
    assert_eq!(
        note.links(),
        vec!["b-note".to_string(), "a-note".to_string()]
    );
}

#[test]
fn an_unterminated_double_bracket_ends_link_scanning() {
    let note = Note::new(
        name("n"),
        valid_frontmatter(),
        "See [[a-note]] then [[broken".to_string(),
    )
    .expect("valid note");
    assert_eq!(note.links(), vec!["a-note".to_string()]);
}

#[test]
fn malformed_frontmatter_is_refused_distinctly() {
    let err = Note::parse(name("n"), "not frontmatter at all").unwrap_err();
    assert!(matches!(err, NoteError::MalformedFrontmatter(_)));
}

// ---------------------------------------------------------------------
// Frontmatter field order, duplicates, and missing-outright fields
// ---------------------------------------------------------------------

#[test]
fn frontmatter_fields_are_accepted_in_any_order() {
    // The most ordinary edit a person makes to a note: swapping two
    // lines. All four fields moved from `Note::render`'s canonical order
    // (title, status, updated, sources) here.
    let text = "---\nsources:\n  - notes/a.txt\nupdated: 2026-09-11\ntitle: Weekly sync\nstatus: draft\n---\n\nHello.";
    let parsed = Note::parse(name("weekly-sync"), text).expect("any field order must parse");

    let expected = Note::new(
        name("weekly-sync"),
        Frontmatter {
            title: "Weekly sync".to_string(),
            status: "draft".to_string(),
            updated: "2026-09-11".to_string(),
            sources: vec!["notes/a.txt".to_string()],
        },
        "Hello.".to_string(),
    )
    .unwrap();

    assert_eq!(parsed, expected);
}

#[test]
fn a_reordered_note_still_survives_the_render_parse_round_trip() {
    let text = "---\nstatus: draft\ntitle: Weekly sync\nsources:\n  - notes/a.txt\nupdated: 2026-09-11\n---\n\nHello.";
    let parsed = Note::parse(name("weekly-sync"), text).expect("any field order must parse");
    let round_tripped =
        Note::parse(name("weekly-sync"), &parsed.render()).expect("renders back cleanly");
    assert_eq!(parsed, round_tripped);
}

#[test]
fn refuses_a_duplicate_title_field() {
    let text = "---\ntitle: One\ntitle: Two\nstatus: draft\nupdated: 2026-09-11\nsources:\n  - notes/a.txt\n---\n\nHello.";
    let err = Note::parse(name("n"), text).unwrap_err();
    assert!(matches!(err, NoteError::MalformedFrontmatter(_)));
}

#[test]
fn refuses_a_duplicate_sources_field() {
    let text = "---\ntitle: One\nstatus: draft\nupdated: 2026-09-11\nsources:\n  - notes/a.txt\nsources:\n  - notes/b.txt\n---\n\nHello.";
    let err = Note::parse(name("n"), text).unwrap_err();
    assert!(matches!(err, NoteError::MalformedFrontmatter(_)));
}

#[test]
fn refuses_frontmatter_with_a_field_missing_outright() {
    // Not an empty `status:` line (that is `validate`'s `MissingField`) —
    // the line is entirely absent from the frontmatter block.
    let text = "---\ntitle: One\nupdated: 2026-09-11\nsources:\n  - notes/a.txt\n---\n\nHello.";
    let err = Note::parse(name("n"), text).unwrap_err();
    assert!(matches!(err, NoteError::MalformedFrontmatter(_)));
}

#[test]
fn refuses_an_unrecognised_frontmatter_key() {
    // Do not start accepting keys `Note::render` never wrote.
    let text = "---\ntitle: One\nstatus: draft\nupdated: 2026-09-11\nauthor: someone\nsources:\n  - notes/a.txt\n---\n\nHello.";
    let err = Note::parse(name("n"), text).unwrap_err();
    assert!(matches!(err, NoteError::MalformedFrontmatter(_)));
}
