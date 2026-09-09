//! Check 2's schema half (ADR 0018 decision 2), from the read-only side:
//! whether the database this build opened is behind, at, or ahead of the
//! schema this build understands.
//!
//! [`latest_schema_version`] is `factory-store`'s own answer, counted from
//! its migration list. This crate deliberately keeps no copy of that number:
//! a schema version written down in two places is two places to update, and
//! the second one is updated late. Individual migrations stay private to
//! `factory-store` — nothing out here is meant to see them, only to ask how
//! far they go.
//!
//! Reading it costs nothing and needs no database, which matters: doctor must
//! answer even when the daemon is down and the file is unopenable.

use factory_store::latest_schema_version;

/// How `database_schema` compares to [`latest_schema_version`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchemaComparison {
    UpToDate,
    Behind,
    Ahead,
}

pub(crate) fn compare(database_schema: i64) -> SchemaComparison {
    use std::cmp::Ordering;
    match database_schema.cmp(&latest_schema_version()) {
        Ordering::Equal => SchemaComparison::UpToDate,
        Ordering::Less => SchemaComparison::Behind,
        Ordering::Greater => SchemaComparison::Ahead,
    }
}

#[cfg(test)]
mod tests {
    use super::{SchemaComparison, compare, latest_schema_version};

    /// The canary described in this module's own doc comment: this crate's
    /// copy of the latest schema number must track `factory-store`'s real
    /// `migrations()` list, and this is the one place that is checked
    /// mechanically rather than by a human remembering to update a comment.
    #[test]
    fn a_freshly_migrated_database_matches_the_known_latest_schema() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = factory_store::Store::open_at(dir.path().join("factory.sqlite"))
            .expect("open a fresh store, migrating it to this build's latest schema");
        assert_eq!(
            store.schema_version().expect("schema_version"),
            latest_schema_version(),
            "factory-store's migrations() grew (or shrank) without this crate's own \
             copy of the latest schema number being updated to match"
        );
    }

    #[test]
    fn compares_behind_up_to_date_and_ahead_correctly() {
        assert_eq!(
            compare(latest_schema_version() - 1),
            SchemaComparison::Behind
        );
        assert_eq!(compare(latest_schema_version()), SchemaComparison::UpToDate);
        assert_eq!(
            compare(latest_schema_version() + 1),
            SchemaComparison::Ahead
        );
    }
}
