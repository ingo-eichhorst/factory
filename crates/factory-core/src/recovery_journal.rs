//! Process receipt I/O for an explicit offline operator action. No daemon,
//! socket, run status, health sampling, scheduling or deployment is involved.
use crate::error::{FactoryError, Result};
use chrono::Utc;
pub use factory_kernel::{ScriptRecoveryAction, ScriptRecoveryFinish};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const MAX_RECEIPT_BYTES: u64 = 32 * 1024;
pub const MAX_INBOX_ACTIONS: usize = 4096;

fn bad(message: &str) -> FactoryError {
    FactoryError::BadRequest(message.into())
}
fn io(error: impl std::fmt::Display) -> FactoryError {
    FactoryError::adapter("recovery journal", error.to_string())
}
pub fn inbox(root: &Path) -> PathBuf {
    root.join(".factory/recovery-outbox")
}

pub fn validate(action: &ScriptRecoveryAction) -> Result<()> {
    let id =
        uuid::Uuid::parse_str(&action.id).map_err(|_| bad("recovery action id must be a UUID"))?;
    if id.to_string() != action.id {
        return Err(bad("recovery action id must be a canonical UUID"));
    }
    for (value, max) in [
        (&action.scope, 200),
        (&action.environment, 200),
        (&action.source, 200),
        (&action.actor, 200),
        (&action.reason, 4000),
        (&action.command, 4000),
    ] {
        if value.trim().is_empty() || value.len() > max {
            return Err(bad("recovery receipt has an empty or oversized field"));
        }
    }
    if action
        .expected_commit
        .as_ref()
        .is_some_and(|commit| commit.trim().is_empty() || commit.len() > 200)
    {
        return Err(bad("recovery installed-commit metadata is invalid"));
    }
    if let Some(finish) = &action.finish {
        if finish.at < action.started_at
            || finish
                .detail
                .as_ref()
                .is_some_and(|detail| detail.len() > 4000)
        {
            return Err(bad(
                "recovery finish has an invalid time or oversized detail",
            ));
        }
    }
    Ok(())
}

fn action_dir(root: &Path, id: &str) -> Result<PathBuf> {
    let id = uuid::Uuid::parse_str(id).map_err(|_| bad("recovery action id must be a UUID"))?;
    Ok(inbox(root).join(id.to_string()))
}

pub fn read(path: &Path) -> Result<ScriptRecoveryAction> {
    if !std::fs::symlink_metadata(path)
        .map_err(io)?
        .file_type()
        .is_file()
    {
        return Err(bad("recovery receipt is not a regular file"));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(io)?
        .take(MAX_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() as u64 > MAX_RECEIPT_BYTES {
        return Err(bad("recovery receipt exceeds the size limit"));
    }
    let action = serde_json::from_slice(&bytes)
        .map_err(|_| bad("recovery receipt is not valid action JSON"))?;
    validate(&action)?;
    Ok(action)
}

/// Publish a fully synced receipt without ever overwriting an existing one.
fn publish(dir: &Path, phase: &str, action: &ScriptRecoveryAction) -> Result<()> {
    validate(action)?;
    let temp = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let bytes = serde_json::to_vec(action).map_err(io)?;
        if bytes.len() as u64 > MAX_RECEIPT_BYTES {
            return Err(bad("encoded recovery receipt exceeds the size limit"));
        }
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)
            .map_err(io)?;
        file.write_all(&bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        std::fs::hard_link(&temp, dir.join(phase)).map_err(io)?;
        std::fs::File::open(dir)
            .and_then(|directory| directory.sync_all())
            .map_err(io)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(temp);
    result
}

pub fn start(root: &Path, mut action: ScriptRecoveryAction) -> Result<ScriptRecoveryAction> {
    if !root.join(".factory/config.yaml").is_file()
        || !root.join(".factory/factory.sqlite").is_file()
    {
        return Err(bad(
            "offline recovery needs an existing instance root/database, not a scope directory",
        ));
    }
    action.id = uuid::Uuid::new_v4().to_string();
    action.started_at = Utc::now();
    action.finish = None;
    validate(&action)?;
    let dir = action_dir(root, &action.id)?;
    std::fs::create_dir_all(inbox(root)).map_err(io)?;
    if !std::fs::symlink_metadata(inbox(root))
        .map_err(io)?
        .file_type()
        .is_dir()
    {
        return Err(bad("recovery outbox must not be a symlink"));
    }
    std::fs::create_dir(&dir).map_err(io)?;
    publish(&dir, "started.json", &action)?;
    // Make the new action directory durable too, before the script acts.
    std::fs::File::open(inbox(root))
        .and_then(|directory| directory.sync_all())
        .map_err(io)?;
    std::fs::File::open(root.join(".factory"))
        .and_then(|directory| directory.sync_all())
        .map_err(io)?;
    Ok(action)
}

pub fn finish(
    root: &Path,
    id: &str,
    mut finish: ScriptRecoveryFinish,
) -> Result<ScriptRecoveryAction> {
    let queued = action_dir(root, id)?;
    let dir = if std::fs::symlink_metadata(&queued)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        root.join(".factory/recovery-receipts").join(
            uuid::Uuid::parse_str(id)
                .map_err(|_| bad("recovery action id must be a UUID"))?
                .to_string(),
        )
    } else {
        queued
    };
    if !std::fs::symlink_metadata(&dir)
        .map_err(io)?
        .file_type()
        .is_dir()
    {
        return Err(bad("recovery action directory is not a regular directory"));
    }
    let mut action = read(&dir.join("started.json"))?;
    if action.id != id || action.finish.is_some() {
        return Err(bad("recovery start does not match this action"));
    }
    if dir.join("finished.json").exists() {
        let existing = read(&dir.join("finished.json"))?;
        let mut baseline = existing.clone();
        baseline.finish = None;
        if baseline != action {
            return Err(bad("recovery finish identity mismatch"));
        }
        let same = existing.finish.as_ref().is_some_and(|old| {
            old.exit_code == finish.exit_code
                && old.local_http == finish.local_http
                && old.network_routes == finish.network_routes
                && old.detail == finish.detail
        });
        if same {
            return Ok(existing);
        }
        return Err(bad(
            "a recovery finish is immutable; an outcome cannot be rewritten",
        ));
    }
    finish.at = Utc::now();
    action.finish = Some(finish);
    publish(&dir, "finished.json", &action)?;
    Ok(action)
}

/// Retain completed receipts after their exact contents have been imported.
/// Pending/invalid actions stay queued. Never delete or overwrite a receipt.
pub fn archive(root: &Path, action: &ScriptRecoveryAction) -> Result<()> {
    if action.finish.is_none() {
        return Ok(());
    }
    let dir = action_dir(root, &action.id)?;
    match std::fs::symlink_metadata(&dir) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(bad("recovery action directory is not a regular directory"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io(error)),
        _ => {}
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(io(error)),
    };
    // A writer removes its temporary link only after publication/sync has
    // completed. Do not race that writer by moving its directory first.
    for entry in entries {
        if entry
            .map_err(io)?
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")
        {
            return Ok(());
        }
    }
    if read(&dir.join("finished.json"))? != *action {
        return Err(bad("recovery receipt changed before archival"));
    }
    let archive = root.join(".factory/recovery-receipts");
    std::fs::create_dir_all(&archive).map_err(io)?;
    if !std::fs::symlink_metadata(&archive)
        .map_err(io)?
        .file_type()
        .is_dir()
    {
        return Err(bad("recovery archive must not be a symlink"));
    }
    let target = archive.join(&action.id);
    if std::fs::symlink_metadata(&target).is_ok() {
        return Err(bad("recovery archive destination already exists"));
    }
    match std::fs::rename(&dir, target) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()), // another importer archived it
        Err(error) => return Err(io(error)),
    }
    for parent in [archive, inbox(root), root.join(".factory")] {
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(io)?;
    }
    Ok(())
}

/// One bounded scan. Corrupt/symlinked/mismatched receipts are counted, not
/// accepted as evidence or allowed to hide valid actions beside them.
pub fn load(root: &Path) -> Result<(Vec<ScriptRecoveryAction>, usize)> {
    match std::fs::symlink_metadata(inbox(root)) {
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(bad("recovery outbox must be a regular directory"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], 0)),
        Err(error) => return Err(io(error)),
        _ => {}
    }
    let entries = match std::fs::read_dir(inbox(root)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((vec![], 0)),
        Err(error) => return Err(io(error)),
    };
    let mut actions = Vec::new();
    let mut rejected = 0;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_INBOX_ACTIONS {
            return Err(bad("recovery outbox exceeds the bounded import limit"));
        }
        let loaded = (|| {
            let entry = entry.map_err(io)?;
            if !entry.file_type().map_err(io)?.is_dir() {
                return Err(bad("recovery action entry is not a directory"));
            }
            let start = read(&entry.path().join("started.json"))?;
            if entry.file_name() != std::ffi::OsStr::new(&start.id) || start.finish.is_some() {
                return Err(bad("recovery start identity mismatch"));
            }
            let final_path = entry.path().join("finished.json");
            if std::fs::symlink_metadata(&final_path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                return Ok((start, false));
            }
            match read(&final_path) {
                Ok(finished) => {
                    let mut baseline = finished.clone();
                    baseline.finish = None;
                    if baseline == start && finished.finish.is_some() {
                        Ok((finished, false))
                    } else {
                        Ok((start, true))
                    }
                }
                Err(_) => Ok((start, true)),
            }
        })();
        match loaded {
            Ok((action, bad_finish)) => {
                actions.push(action);
                rejected += usize::from(bad_finish);
            }
            Err(_) => rejected += 1,
        }
    }
    Ok((actions, rejected))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("factory-offline-recovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory")).unwrap();
        std::fs::write(root.join(".factory/config.yaml"), "version: 1\n").unwrap();
        std::fs::write(root.join(".factory/factory.sqlite"), b"").unwrap();
        root
    }
    fn action() -> ScriptRecoveryAction {
        ScriptRecoveryAction {
            id: String::new(),
            scope: "demo".into(),
            environment: "production".into(),
            source: "ensure.sh".into(),
            actor: "operator".into(),
            reason: "daemon not running".into(),
            command: "restart installed daemon".into(),
            started_at: Utc::now(),
            expected_commit: None,
            finish: None,
        }
    }
    fn outcome(code: u8) -> ScriptRecoveryFinish {
        ScriptRecoveryFinish {
            at: Utc::now(),
            exit_code: code,
            local_http: Some(true),
            network_routes: Some(code == 0),
            detail: None,
        }
    }
    #[test]
    fn offline_start_and_finish_are_immutable_durable_and_retryable() {
        let root = root();
        let started = start(&root, action()).unwrap();
        assert_eq!(load(&root).unwrap().0, vec![started.clone()]);
        let ended = finish(&root, &started.id, outcome(1)).unwrap();
        assert_eq!(load(&root).unwrap().0, vec![ended.clone()]);
        assert_eq!(finish(&root, &started.id, outcome(1)).unwrap(), ended);
        assert!(finish(&root, &started.id, outcome(0)).is_err());
        assert!(finish(&root, "../elsewhere", outcome(0)).is_err());
        archive(&root, &ended).unwrap();
        assert!(load(&root).unwrap().0.is_empty());
        assert_eq!(finish(&root, &started.id, outcome(1)).unwrap(), ended);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn corrupt_or_forged_receipts_do_not_hide_good_actions() {
        let root = root();
        let good = start(&root, action()).unwrap();
        let other = start(&root, action()).unwrap();
        std::fs::write(
            action_dir(&root, &other.id).unwrap().join("finished.json"),
            b"not JSON",
        )
        .unwrap();
        let (actions, rejected) = load(&root).unwrap();
        assert_eq!(actions.len(), 2);
        assert!(actions.contains(&good) && actions.contains(&other));
        assert_eq!(rejected, 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archive_defers_active_publication_and_preserves_an_existing_destination() {
        let root = root();
        let started = start(&root, action()).unwrap();
        let ended = finish(&root, &started.id, outcome(0)).unwrap();
        let dir = action_dir(&root, &started.id).unwrap();
        let temporary = dir.join(".publisher.tmp");
        std::fs::write(&temporary, b"pending sync").unwrap();
        archive(&root, &ended).unwrap();
        assert!(dir.exists());
        std::fs::remove_file(temporary).unwrap();
        let target = root.join(".factory/recovery-receipts").join(&started.id);
        std::fs::create_dir_all(&target).unwrap();
        assert!(archive(&root, &ended).is_err());
        assert!(dir.exists() && target.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_scope_directory_cannot_become_a_runtime_journal_root() {
        let root = root();
        let scope = root.join("projects/demo");
        std::fs::create_dir_all(scope.join(".factory")).unwrap();
        std::fs::write(
            scope.join(".factory/config.yaml"),
            "scope: { id: demo, name: demo }\n",
        )
        .unwrap();
        assert!(start(&scope, action())
            .unwrap_err()
            .to_string()
            .contains("not a scope directory"));
        assert!(!inbox(&scope).exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_or_identity_changed_finishes_keep_a_valid_start_unknown() {
        let root = root();
        let started = start(&root, action()).unwrap();
        let final_path = action_dir(&root, &started.id)
            .unwrap()
            .join("finished.json");
        std::fs::write(&final_path, vec![b'x'; MAX_RECEIPT_BYTES as usize + 1]).unwrap();
        assert_eq!(load(&root).unwrap(), (vec![started.clone()], 1));
        let mut forged = started.clone();
        forged.actor = "different".into();
        forged.finish = Some(outcome(0));
        std::fs::write(&final_path, serde_json::to_vec(&forged).unwrap()).unwrap();
        assert_eq!(load(&root).unwrap(), (vec![started.clone()], 1));
        assert!(finish(&root, &started.id, outcome(0)).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_receipt_entries_and_outboxes_are_not_followed() {
        let root = root();
        let started = start(&root, action()).unwrap();
        let dir = action_dir(&root, &started.id).unwrap();
        std::os::unix::fs::symlink(dir.join("started.json"), dir.join("finished.json")).unwrap();
        assert_eq!(load(&root).unwrap(), (vec![started], 1));
        std::fs::rename(inbox(&root), root.join("archived")).unwrap();
        std::os::unix::fs::symlink(root.join("archived"), inbox(&root)).unwrap();
        assert!(load(&root).is_err() && start(&root, action()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
