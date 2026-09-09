//! `factory doctor` (design §7; backlog §10): read-only, no daemon required
//! — [`factory_doctor::diagnose`] never opens the socket. Binding decision 3:
//! a finding means a non-zero exit, so this is usable from a check without
//! parsing its output; the exit code alone already answers "is anything
//! wrong", and the printed report answers "what, and what do I do about it."

use std::path::Path;

use factory_doctor::{DoctorReport, Finding};

use crate::exit;

pub fn run(root: &Path) -> i32 {
    match factory_doctor::diagnose(root) {
        Err(source) => {
            eprintln!(
                "factory: could not diagnose {}: {source}\n  help: run `factory init --root {}` \
                 if this instance was never created",
                root.display(),
                root.display()
            );
            exit::DOCTOR_CANT_DIAGNOSE
        }
        Ok(report) => {
            print_report(&report);
            if report.is_healthy() {
                exit::OK
            } else {
                exit::DOCTOR_FINDINGS
            }
        }
    }
}

fn print_report(report: &DoctorReport) {
    println!("schema_version: {}", report.schema_version);
    println!("integrity_check: {}", report.integrity_check);
    println!(
        "backups: count={} total_size_bytes={} oldest={:?} newest={:?}",
        report.backups.count,
        report.backups.total_size_bytes,
        report.backups.oldest_modified,
        report.backups.newest_modified
    );
    println!(
        "scheduler: label={} loaded={} last_fired={:?}",
        report.scheduler.label, report.scheduler.loaded, report.scheduler.last_fired
    );

    if report.findings.is_empty() {
        println!("findings: none");
        return;
    }

    println!("findings ({}):", report.findings.len());
    for finding in &report.findings {
        println!("- {}", render(finding));
    }
}

/// One line per finding: what is wrong, the stable ids involved, and what to
/// do — the acceptance standard this crate's brief names explicitly.
fn render(finding: &Finding) -> String {
    match finding {
        Finding::ConfigInvalid(error) => format!(
            "config invalid: {error}\n  help: fix `.factory/config.yaml`, then run `factory doctor` again"
        ),
        Finding::IntegrityCheckFailed(verdict) => format!(
            "database integrity_check failed: {verdict}\n  help: restore a snapshot from `.factory/backups/`"
        ),
        Finding::SchemaBehindBuild {
            database_schema,
            built_schema,
        } => format!(
            "schema behind build: database is at schema {database_schema}, this build understands \
             schema {built_schema}\n  help: run any command that opens the database read-write \
             (e.g. `factory start`) to migrate it"
        ),
        Finding::SchemaAheadOfBuild {
            database_schema,
            built_schema,
        } => format!(
            "schema ahead of build: database is at schema {database_schema}, this build only \
             understands schema {built_schema}\n  help: install a newer build of factory"
        ),
        Finding::CheckSkipped { check, reason } => {
            format!("check skipped: {check:?}: {reason}")
        }
        Finding::RegistryDrift(drift) => format!(
            "registry drift: {drift:?}\n  help: review the drift, then run `factory scope reconcile \
             --apply` if it should be applied"
        ),
        Finding::RegistryUnresolvable(error) => format!(
            "registry unresolvable: {error}\n  help: check the scope's declared path in \
             `.factory/config.yaml`"
        ),
        Finding::OrphanedLease {
            session_id,
            agent_name,
            session_state,
            workspace_path,
            lease_id,
        } => format!(
            "orphaned lease {lease_id}: session {session_id} ({agent_name}, state {session_state}) \
             holds workspace {workspace_path} with no live claim\n  help: investigate session \
             {session_id}; a stuck lease blocks other sessions from that workspace"
        ),
        Finding::SessionPaneGone {
            session_id,
            pane_id,
        } => format!(
            "session {session_id}'s pane {pane_id} no longer exists\n  help: stop or restart session \
             {session_id}"
        ),
        Finding::PaneStillAliveForDeadSession {
            session_id,
            pane_id,
        } => format!(
            "pane {pane_id} is still alive, but session {session_id} is not recorded as live\n  \
             help: close pane {pane_id} by hand if it is no longer wanted"
        ),
        Finding::HerdrPaneQueryFailed(error) => format!(
            "could not query herdr's panes: {error}\n  help: confirm herdr is installed and on PATH"
        ),
        Finding::SchedulerNotLoaded { label } => format!(
            "scheduler job `{label}` is not loaded\n  help: load it if a recurring dispatcher is \
             expected on this machine"
        ),
        Finding::SchedulerQueryFailed { label, error } => format!(
            "could not query scheduler job `{label}`: {error}\n  help: confirm launchctl is on PATH"
        ),
    }
}
