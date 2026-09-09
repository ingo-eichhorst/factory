//! Check 2's backup half (ADR 0019 decision 5): "the count, the total size,
//! and the oldest and newest snapshot" of `.factory/backups/`. Reported,
//! never acted on — Factory deletes nothing from that directory (ADR 0012
//! decision 4, restated by ADR 0019 decision 5), and no size or age
//! threshold anywhere makes this directory's growth itself a finding: this
//! module always produces a [`BackupsSummary`], never a [`crate::Finding`].

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What ADR 0019 decision 5 asks doctor to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupsSummary {
    pub count: usize,
    pub total_size_bytes: u64,
    pub oldest_modified: Option<SystemTime>,
    pub newest_modified: Option<SystemTime>,
}

/// `.factory/backups/`: the directory named `backups` beside the database
/// file. Mirrors `factory_store::snapshot::backups_dir` (private to that
/// crate) as one line rather than a dependency on it — for the company-root
/// database, `db_path`'s parent is already `.factory/`, so this lands
/// exactly on `.factory/backups/`.
fn backups_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("backups")
}

/// Summarize `db_path`'s backup directory.
///
/// A missing directory — no migration has ever run, or the instance is
/// fresh — summarizes as all-zero, not an error: ADR 0018 decision 3 exempts
/// a fresh database from ever getting a pre-migration snapshot, so an
/// instance that has never upgraded legitimately has no `backups/` at all.
/// Any other read failure (permissions, a non-directory occupying the path)
/// degrades the same way, because this is one fact among six in a read-only
/// pass and must not abort the other checks.
pub(crate) fn summarize(db_path: &Path) -> BackupsSummary {
    let dir = backups_dir(db_path);

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return BackupsSummary {
            count: 0,
            total_size_bytes: 0,
            oldest_modified: None,
            newest_modified: None,
        };
    };

    let mut count = 0usize;
    let mut total_size_bytes = 0u64;
    let mut oldest: Option<SystemTime> = None;
    let mut newest: Option<SystemTime> = None;

    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        count += 1;
        total_size_bytes += metadata.len();
        if let Ok(modified) = metadata.modified() {
            oldest = Some(oldest.map_or(modified, |current| current.min(modified)));
            newest = Some(newest.map_or(modified, |current| current.max(modified)));
        }
    }

    BackupsSummary {
        count,
        total_size_bytes,
        oldest_modified: oldest,
        newest_modified: newest,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::summarize;

    #[test]
    fn a_missing_backups_directory_summarizes_as_all_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join(".factory").join("factory.sqlite");

        let summary = summarize(&db_path);

        assert_eq!(summary.count, 0);
        assert_eq!(summary.total_size_bytes, 0);
        assert_eq!(summary.oldest_modified, None);
        assert_eq!(summary.newest_modified, None);
    }

    /// Mutation target: summing sizes with the wrong accumulator, or
    /// comparing mtimes backwards (`max` where `min` belongs or vice versa).
    /// Two files with known sizes and known, deliberately out-of-creation-
    /// order mtimes pin both halves: the total must be the sum, and "oldest"
    /// must be the earlier timestamp regardless of which file was written to
    /// disk first.
    #[test]
    fn counts_files_sums_sizes_and_finds_the_oldest_and_newest_by_mtime() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join(".factory").join("factory.sqlite");
        let backups = dir.path().join(".factory").join("backups");
        std::fs::create_dir_all(&backups).expect("create backups dir");

        // Written in this order, but stamped with mtimes in the *opposite*
        // order, so a test that accidentally checked creation order instead
        // of mtime would fail loudly rather than passing by coincidence.
        let first_written = backups.join("pre-migration-1-to-2-a.sqlite");
        let second_written = backups.join("pre-migration-2-to-3-b.sqlite");
        std::fs::write(&first_written, vec![0u8; 10]).expect("write first");
        std::fs::write(&second_written, vec![0u8; 20]).expect("write second");

        let later_time = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(2_000);
        let earlier_time = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        std::fs::File::open(&first_written)
            .expect("open first")
            .set_modified(later_time)
            .expect("set mtime on the file written first, to the later timestamp");
        std::fs::File::open(&second_written)
            .expect("open second")
            .set_modified(earlier_time)
            .expect("set mtime on the file written second, to the earlier timestamp");

        let summary = summarize(&db_path);

        assert_eq!(summary.count, 2);
        assert_eq!(summary.total_size_bytes, 30);
        assert_eq!(summary.oldest_modified, Some(earlier_time));
        assert_eq!(summary.newest_modified, Some(later_time));
    }

    #[test]
    fn a_non_directory_occupying_the_backups_path_summarizes_as_all_zero_rather_than_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join(".factory").join("factory.sqlite");
        std::fs::create_dir_all(db_path.parent().unwrap()).expect("create .factory");
        std::fs::write(
            db_path.parent().unwrap().join("backups"),
            b"not a directory",
        )
        .expect("occupy the backups path with a plain file");

        let summary = summarize(&db_path);

        assert_eq!(summary.count, 0);
    }
}
