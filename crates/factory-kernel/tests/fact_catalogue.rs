//! Guards phase 2 of #193: every fact behind `policy::Evidence` is
//! catalogued with a producer and its readers, and the four fact types that
//! moved into the kernel outright actually implement `Fact` with the
//! producer the catalogue claims for them. The other seven still live in
//! `factory-core` -- this crate cannot name their types (the kernel depends
//! on no `factory-*` crate, guarded by `no_factory_dependency.rs`), so the
//! catalogue is the one place that documents them all in one list; their
//! own `impl Fact` sits beside each type where it is defined.

use factory_kernel::{BackupFact, DaemonConfigFact, Fact, Level, SecretsPresence, FACT_CATALOGUE, L1, L2};

/// A fact whose `Producer` is exactly `P` -- fails to compile if a moved
/// type's `impl Fact` ever drifts from what [`FACT_CATALOGUE`] claims for
/// it.
fn assert_producer<F: Fact<Producer = P>, P: Level>() {}

#[test]
fn moved_facts_implement_fact_with_the_producer_the_catalogue_claims() {
    assert_producer::<DaemonConfigFact, L1>();
    assert_producer::<BackupFact, L1>();
    assert_producer::<SecretsPresence, L2>();
}

#[test]
fn every_catalogued_fact_names_a_real_level_and_at_least_one_reader() {
    let known_levels = ["L1", "L2", "L3", "L4", "L5", "L6"];
    for entry in FACT_CATALOGUE {
        assert!(
            known_levels.contains(&entry.producer),
            "{}'s producer {:?} is not one of {known_levels:?}",
            entry.fact,
            entry.producer,
        );
        assert!(!entry.readers.is_empty(), "{} names no reader", entry.fact);
        assert!(
            !entry.note.trim().is_empty(),
            "{} carries no note explaining where it lives",
            entry.fact
        );
    }
}

#[test]
fn every_catalogued_fact_name_is_unique() {
    let mut names: Vec<&str> = FACT_CATALOGUE.iter().map(|entry| entry.fact).collect();
    let before = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), before, "FACT_CATALOGUE repeats a fact name");
}

/// The catalogue's own `lives_in_kernel` flag must agree with what actually
/// implements `Fact` here -- exactly the three this crate can name plus
/// `VerifySummary`, `BackupFact`'s own nested fact, which the catalogue
/// does not list separately (it travels with `BackupFact`).
#[test]
fn exactly_the_facts_this_crate_can_name_are_marked_as_living_here() {
    let in_kernel: Vec<&str> = FACT_CATALOGUE.iter().filter(|e| e.lives_in_kernel).map(|e| e.fact).collect();
    let mut expected = vec!["DaemonConfigFact", "BackupFact", "SecretsPresence"];
    let mut in_kernel_sorted = in_kernel.clone();
    in_kernel_sorted.sort_unstable();
    expected.sort_unstable();
    assert_eq!(in_kernel_sorted, expected, "FACT_CATALOGUE's lives_in_kernel flags do not match this module's own facts");
}

#[test]
fn the_catalogue_lists_every_fact_behind_policy_evidence() {
    let names: Vec<&str> = FACT_CATALOGUE.iter().map(|e| e.fact).collect();
    for expected in [
        "DaemonConfigFact",
        "BackupFact",
        "SecretsPresence",
        "DependenciesFact",
        "ExploitedFinding",
        "TaskFact",
        "WorkflowFact",
        "ConfirmedSecurityReport",
        "GateFact",
        "AgentFact",
    ] {
        assert!(names.contains(&expected), "FACT_CATALOGUE is missing {expected:?}");
    }
}
