//! A ratchet on cross-level reach inside the daemon (#193 phase 6, slice S2).
//!
//! `Engine` keeps each level's state in its own group (`self.l4.store`). A
//! source file owned by level N must not name another level's group. Today
//! some do; those are listed in `BASELINE`, and that list may only shrink: a new
//! violation fails the test, and so does a fixed one that is still listed (lower
//! the number or delete the line). The same count applies to `Facts::<People>`,
//! which belongs to the router and page composition, not to a level (the
//! `router/` files are that side, so they are exempt).
//!
//! This is textual, deliberately: the daemon is one crate, so cargo cannot
//! enforce what the level crates already enforce between themselves. Test code
//! (a `#[cfg(test)] mod`, and `tests.rs`/`*_tests.rs` files) and comments are not scanned: fixtures build whole instances.
//!
//! Every source file must be placed in `OWNERS`, so a new file cannot dodge the
//! check by being unlisted.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Owner {
    L1,
    L2,
    L3,
    L4,
    L5,
    L6,
    /// The entry point, wiring and page-side code that may touch any group
    /// until the router split and the services replace it (S3 to S12).
    /// `engine.rs` is in this class today although most of it is L4 dispatch;
    /// it leaves it as its methods move into services.
    Wiring,
}

use Owner::*;

/// Path prefix (a file, or a directory ending in `/`) -> owner. Where a module
/// is a page rather than a service the owner decides later (D5); it is filed
/// under the level whose state it mostly reads.
const OWNERS: &[(&str, Owner)] = &[
    // L1 Infrastructure
    ("backup/", L1),
    ("environments/", L1),
    ("host.rs", L1),
    ("host_power.rs", L1),
    ("host_power/", L1),
    ("power.rs", L1),
    ("github_deployments.rs", L1),
    ("doctor.rs", L1),
    ("renewals/", L1), // also touches L2 and L6 state, see BASELINE
    // L2 Environment
    ("provision.rs", L2),
    ("provision/", L2),
    ("secrets.rs", L2),
    ("secrets/", L2),
    ("dependencies.rs", L2),
    ("openshell.rs", L2),
    ("service_observations.rs", L2),
    // L3 Agent
    ("agents.rs", L3),
    ("roles.rs", L3),
    ("harness_health.rs", L3),
    // L4 Process
    ("workflows/", L4),
    ("intake.rs", L4),
    ("github_intake.rs", L4),
    ("github_outbound.rs", L4),
    ("verification.rs", L4),
    ("waiting.rs", L4),
    ("scheduler.rs", L4),
    ("schedule.rs", L4),
    ("occupancy.rs", L4),
    ("operations.rs", L4),
    ("costs.rs", L4),
    ("workspace_lifecycle.rs", L4),
    ("worktree.rs", L4),
    ("worktree/", L4),
    ("artifacts.rs", L4),
    ("production.rs", L4),
    ("recovery_journal.rs", L4),
    ("assignments.rs", L4),
    ("resume.rs", L4),
    ("site.rs", L4),
    // L5 Improvement
    ("bench/", L5),
    ("datasets.rs", L5),
    ("suggestions.rs", L5),
    ("quality/", L5),
    ("metrics.rs", L5),
    ("signposts.rs", L5),
    // L6 Direction
    ("policies/", L6),
    ("goals/", L6),
    ("scenarios/", L6),
    ("budgets.rs", L6),
    ("l6_service.rs", L6),
    // The entry point, wiring and shared page-side code.
    ("router/l1.rs", L1),
    ("router/l2.rs", L2),
    ("router/l3.rs", L3),
    ("router/l4.rs", L4),
    ("router/l5.rs", L5),
    ("router/l6.rs", L6),
    ("router/mod.rs", Wiring),
    ("router/own.rs", Wiring),
    ("engine.rs", Wiring),
    ("access.rs", Wiring),
    ("main.rs", Wiring),
    ("state.rs", Wiring),
    ("interfaces/", Wiring),
    ("facts/", Wiring),
    ("stores.rs", Wiring),
    ("commands.rs", Wiring),
    ("configuration.rs", Wiring),
    ("discovery.rs", Wiring),
    ("ui.rs", Wiring),
];

/// (file, group, count): today's cross-level reach. May only shrink.
const BASELINE: &[(&str, &str, usize)] = &[
    ("agents.rs", "l4", 26),
    ("bench/engine.rs", "l4", 8),
    ("datasets.rs", "l4", 2),
    ("dependencies.rs", "l4", 2),
    ("doctor.rs", "l2", 2),
    ("environments/mod.rs", "l4", 2),
    ("github_deployments.rs", "l4", 4),
    ("harness_health.rs", "l4", 7),
    ("host_power.rs", "l4", 2),
    ("operations.rs", "l2", 1),
    ("operations.rs", "l3", 1),
    ("renewals/mod.rs", "l2", 2),
    ("renewals/mod.rs", "l6", 4),
    ("roles.rs", "l4", 1),
    ("secrets.rs", "l4", 2),
    ("suggestions.rs", "l4", 3),
];

/// A level service and the accessor that enters it (`engine.l6_service()`).
struct ServiceEntry {
    owner: Owner,
    accessor: &'static str,
}
const SERVICES: &[ServiceEntry] = &[ServiceEntry { owner: L6, accessor: ".l6_service()" }];

/// (file -> accessor, count): modules that reach into a level service from
/// another level. Each is a pull the owning slice has to replace. May only shrink.
const PULLS_BASELINE: &[(&str, usize)] = &[
    ("metrics.rs -> .l6_service()", 3),
    ("quality/mod.rs -> .l6_service()", 1),
    ("renewals/mod.rs -> .l6_service()", 1),
    ("signposts.rs -> .l6_service()", 2),
    ("verification.rs -> .l6_service()", 2),
];

/// Files that name `Facts::<People>` today (count). May only shrink.
const PEOPLE_BASELINE: &[(&str, usize)] = &[
    ("environments/mod.rs", 3),
    ("policies/mod.rs", 1),
    ("production.rs", 1),
    ("signposts.rs", 1),
];

fn src_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn owner_of(relative: &str) -> Option<Owner> {
    OWNERS
        .iter()
        .find(|(prefix, _)| {
            if prefix.ends_with('/') {
                relative.starts_with(prefix)
            } else {
                relative == *prefix
            }
        })
        .map(|(_, owner)| *owner)
}

fn is_test_file(relative: &str) -> bool {
    let name = relative.rsplit('/').next().unwrap();
    name == "tests.rs" || name.ends_with("_tests.rs")
}

/// Production code only, comments blanked.
fn production_code(source: &str) -> String {
    // The test module, not an item that is only compiled for tests.
    let mut end = source.len();
    let mut from = 0;
    while let Some(at) = source[from..].find("#[cfg(test)]") {
        let at = from + at;
        let rest = source[at + "#[cfg(test)]".len()..].trim_start();
        if rest.starts_with("mod ") || rest.starts_with("pub mod ") || rest.starts_with("pub(crate) mod ") {
            end = at;
            break;
        }
        from = at + 1;
    }
    source[..end]
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `(self|engine) . lN` including a chain broken across lines. Strings are not
/// excluded: a production string naming a group would be unusual enough to look at.
fn group_reach(code: &str) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for group in ["l1", "l2", "l3", "l4", "l5", "l6"] {
        let mut count = 0;
        let bytes = code.as_bytes();
        let needle = format!(".{group}");
        let mut from = 0;
        while let Some(at) = code[from..].find(&needle) {
            let at = from + at;
            from = at + needle.len();
            let after = bytes.get(at + needle.len()).copied();
            if after.is_some_and(|b| b.is_ascii_alphanumeric() || b == b'_') {
                continue;
            }
            let before = code[..at].trim_end();
            if before.ends_with("self") || before.ends_with("engine") {
                count += 1;
            }
        }
        if count > 0 {
            counts.insert(group, count);
        }
    }
    counts
}

fn people_reach(code: &str) -> usize {
    ["Facts::<People>", "Facts::<factory_kernel::People>", ".facts::<People>", ".facts::<factory_kernel::People>"]
        .iter()
        .map(|needle| code.matches(needle).count())
        .sum::<usize>()
}

fn level_group(owner: Owner) -> Option<&'static str> {
    Some(match owner {
        L1 => "l1",
        L2 => "l2",
        L3 => "l3",
        L4 => "l4",
        L5 => "l5",
        L6 => "l6",
        Wiring => return None,
    })
}

struct Scan {
    unplaced: Vec<String>,
    reach: BTreeMap<(String, &'static str), usize>,
    people: BTreeMap<String, usize>,
    pulls: BTreeMap<String, usize>,
}

fn scan() -> Scan {
    let root = src_root();
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    files.sort();
    let mut result = Scan { unplaced: Vec::new(), reach: BTreeMap::new(), people: BTreeMap::new(), pulls: BTreeMap::new() };
    for file in files {
        let relative = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if is_test_file(&relative) {
            continue;
        }
        let Some(owner) = owner_of(&relative) else {
            result.unplaced.push(relative);
            continue;
        };
        let Some(own_group) = level_group(owner) else {
            continue; // wiring and the entry point may reach any group
        };
        let code = production_code(&std::fs::read_to_string(&file).unwrap());
        // A level service is entered by its router and by wiring. A lower level's
        // module calling into it is a pull against the ladder.
        for service in SERVICES {
            if service.owner != owner {
                let pulls = code.matches(service.accessor).count();
                if pulls > 0 {
                    result.pulls.insert(format!("{relative} -> {}", service.accessor), pulls);
                }
            }
        }
        for (group, count) in group_reach(&code) {
            if group != own_group {
                result.reach.insert((relative.clone(), group), count);
            }
        }
        // The routers are the people-side of the entry point: they may read `Facts<People>`.
        let people = if relative.starts_with("router/") { 0 } else { people_reach(&code) };
        if people > 0 {
            result.people.insert(relative.clone(), people);
        }
    }
    result
}

fn describe(scan: &Scan) -> String {
    let mut out = String::from("const BASELINE: &[(&str, &str, usize)] = &[\n");
    for ((file, group), count) in &scan.reach {
        out.push_str(&format!("    ({file:?}, {group:?}, {count}),\n"));
    }
    out.push_str("];\nconst PULLS_BASELINE: &[(&str, usize)] = &[\n");
    for (file, count) in &scan.pulls {
        out.push_str(&format!("    ({file:?}, {count}),\n"));
    }
    out.push_str("];\nconst PEOPLE_BASELINE: &[(&str, usize)] = &[\n");
    for (file, count) in &scan.people {
        out.push_str(&format!("    ({file:?}, {count}),\n"));
    }
    out.push_str("];\n");
    out
}

#[test]
fn every_daemon_source_file_has_an_owner() {
    let scan = scan();
    assert!(
        scan.unplaced.is_empty(),
        "place these files in OWNERS (level_ownership_tests.rs): {:?}",
        scan.unplaced
    );
}

#[test]
fn cross_level_reach_only_shrinks() {
    let scan = scan();
    let baseline: BTreeMap<(String, &str), usize> =
        BASELINE.iter().map(|(f, g, n)| ((f.to_string(), *g), *n)).collect();
    let mut grew = Vec::new();
    let mut stale = Vec::new();
    for (key, count) in &scan.reach {
        match baseline.get(&(key.0.clone(), key.1)) {
            None => grew.push(format!("{} reaches {} ({count} sites): new", key.0, key.1)),
            Some(allowed) if count > allowed => {
                grew.push(format!("{} reaches {} ({count} sites > {allowed})", key.0, key.1))
            }
            Some(allowed) if count < allowed => {
                stale.push(format!("{} reaches {}: {count} sites < baseline {allowed}", key.0, key.1))
            }
            _ => {}
        }
    }
    for (file, group) in baseline.keys() {
        if !scan.reach.contains_key(&(file.clone(), group)) {
            stale.push(format!("{file} no longer reaches {group}: remove it from BASELINE"));
        }
    }
    assert!(
        grew.is_empty() && stale.is_empty(),
        "cross-level reach must only shrink.\nnew or grown: {grew:#?}\nfixed (lower the baseline): {stale:#?}\ncurrent state:\n{}",
        describe(&scan)
    );
}

#[test]
fn people_reads_stay_with_the_router_and_only_shrink() {
    let scan = scan();
    let baseline: BTreeMap<&str, usize> = PEOPLE_BASELINE.iter().copied().collect();
    let mut bad = Vec::new();
    for (file, count) in &scan.people {
        match baseline.get(file.as_str()) {
            None => bad.push(format!("{file}: {count} Facts::<People> reads, new")),
            Some(allowed) if count != allowed => {
                bad.push(format!("{file}: {count} Facts::<People> reads, baseline {allowed}"))
            }
            _ => {}
        }
    }
    for file in baseline.keys() {
        if !scan.people.contains_key(*file) {
            bad.push(format!("{file}: no longer reads Facts::<People>, remove it from PEOPLE_BASELINE"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}\ncurrent state:\n{}", describe(&scan));
}

#[test]
fn pulls_into_a_level_service_from_other_levels_only_shrink() {
    let scan = scan();
    let baseline: BTreeMap<&str, usize> = PULLS_BASELINE.iter().copied().collect();
    let mut bad = Vec::new();
    for (key, count) in &scan.pulls {
        match baseline.get(key.as_str()) {
            None => bad.push(format!("{key}: {count} calls, new")),
            Some(allowed) if count != allowed => bad.push(format!("{key}: {count} calls, baseline {allowed}")),
            _ => {}
        }
    }
    for key in baseline.keys() {
        if !scan.pulls.contains_key(*key) {
            bad.push(format!("{key}: no longer called, remove it from PULLS_BASELINE"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}\ncurrent state:\n{}", describe(&scan));
}

#[test]
fn the_scanner_sees_a_reach_and_ignores_comments_and_tests() {
    let code = production_code(
        "#[cfg(test)]\nuse x;\nfn a(&self) { self.l4.store.get(); }\n// self.l5.bench.x\nfn b() { engine\n  .l2.provision.y; }\n#[cfg(test)]\nmod t { fn c() { self.l6.goals.z; } }\n",
    );
    let reach = group_reach(&code);
    assert_eq!(reach.get("l4"), Some(&1));
    assert_eq!(reach.get("l2"), Some(&1));
    assert_eq!(reach.get("l5"), None);
    assert_eq!(reach.get("l6"), None);
    assert_eq!(group_reach("self.l40.x; self.l4_x.y; other.l4.z;").len(), 0);
}
