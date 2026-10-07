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
    ("environments/promotion.rs", Wiring), // composes L4 workflow/promotion over L1 deployments and L2 environments
    ("intent.rs", Wiring), // authored L6 intent as plain input, read by any level
    ("environments/recovery.rs", Wiring), // composes L4 recovery workflows over L1/L2 recovery evidence
    ("environments/report.rs", Wiring), // the Operations environments report: L4 and L2 facts as one page
    ("environments/", L1),
    ("host.rs", L1),
    ("host_power.rs", L1),
    ("host_power/page.rs", Wiring), // the host power page: L1 assertion state beside L4 liveness
    ("host_power/", L1),
    ("power.rs", L1),
    ("github_deployments.rs", Wiring), // mirrors an L1 deployment to GitHub: L1 facts, L4's mirror record and the authorization, composed
    ("doctor.rs", Wiring), // `factory doctor`: diagnostics across every level
    ("renewals/mod.rs", Wiring), // the Important Dates page: L1 and L2 caches plus L6 attestations and clock
    ("renewals/", L1),
    // L2 Environment
    ("provision.rs", L2),
    ("l1_service.rs", L1),
    ("l2_service.rs", L2),
    ("provision/", L2),
    ("secrets.rs", L2),
    ("secrets/", L2),
    ("dependencies.rs", L2),
    ("openshell.rs", L2),
    ("service_observations.rs", L2),
    // L3 Agent
    ("agent_rows.rs", L3),
    ("dispatch_port.rs", L3),
    ("l3_service.rs", L3),
    ("agents.rs", L3),
    ("roles.rs", L3),
    ("harness_health.rs", L3),
    ("harness_hold.rs", L4),
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
    ("occupancy_page.rs", Wiring), // the occupancy chart: L4 runs and liveness composed with L3's roster
    ("operations_report.rs", Wiring), // the Operations tab: L4 tasks and runs composed with L2/L3 attention
    ("operations.rs", L4),
    ("costs.rs", L4),
    ("workspace_lifecycle.rs", L4),
    ("worktree.rs", L4),
    ("worktree/", L4),
    ("artifacts.rs", L4),
    ("production.rs", Wiring), // the People-side production endpoint: reads L4's fact as a page
    ("recovery_journal.rs", L4),
    ("admission.rs", L4),
    ("run_settle.rs", L4),
    ("run_start.rs", L4),
    ("scheduling.rs", L4),
    ("supplied.rs", Wiring), // what L5/L6 supply to L4 (quality block, functionary binding) and workflow lint
    ("l2_pages.rs", Wiring), // L2 catalogues composed with L4 runs and journals: attachments, secret metadata
    ("l4_port.rs", L4),
    ("l4_service.rs", L4),
    ("l4_spawner.rs", L4),
    ("assignments.rs", L4),
    ("resume.rs", L4),
    ("site.rs", Wiring), // the site page: composes the roster and runs with a repository walk
    // L5 Improvement
    ("bench/", L5),
    ("l5_service.rs", L5),
    ("l5_spawner.rs", L5),
    ("datasets.rs", L5),
    ("suggestions.rs", L5),
    ("quality/", Wiring), // L5 quality evaluation composed over metrics and the cross-level check service
    ("metrics.rs", Wiring), // L5 metrics composed with L6 policy inputs over L1 to L4 evidence
    ("signposts.rs", Wiring), // L5 signposts composed the same way
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
    ("router/mod.rs", Wiring), // request routing: the entry point
    ("router/own.rs", Wiring), // routing for requests no level owns
    ("scope_views.rs", Wiring), // scope read views over L3 agents and L4 tasks
    ("runtime_events.rs", Wiring), // L2 runtime push events fanned to L3 agents and L4 runs
    ("engine.rs", Wiring), // the Engine: status and infrastructure composition, construction, the forwarders that remain
    ("access.rs", Wiring), // authorization over every request: reads L3 roles and L4 tasks
    ("main.rs", Wiring), // startup: builds the services and starts the workers
    ("state.rs", Wiring), // the per-level state groups
    ("interfaces/", Wiring), // socket and http: the entry points
    ("facts/", Wiring), // the fact providers: the wiring that serves each level its facts
    ("stores.rs", Wiring), // store construction
    ("commands.rs", Wiring), // the command and port constructors between adjacent levels
    ("configuration.rs", Wiring), // scope configuration edits (dashboard, roles, secrets, policies files)
    ("discovery.rs", Wiring), // instance discovery
    ("ui.rs", Wiring), // the embedded UI
];

/// (file, group, count): today's cross-level reach. May only shrink.
const BASELINE: &[(&str, &str, usize)] = &[
];

/// A level service and the accessor that enters it (`engine.l6_service()`).
struct ServiceEntry {
    owner: Owner,
    accessor: &'static str,
}
const SERVICES: &[ServiceEntry] = &[
    ServiceEntry { owner: L1, accessor: ".l1_service()" },
    ServiceEntry { owner: L2, accessor: ".l2_service()" },
    ServiceEntry { owner: L3, accessor: ".l3_service()" },
    ServiceEntry { owner: L4, accessor: ".l4_service()" },
    ServiceEntry { owner: L5, accessor: ".l5_service()" },
    ServiceEntry { owner: L6, accessor: ".l6_service()" },
];

/// (file -> accessor, count): modules that reach into a level service from
/// another level. Each is a pull the owning slice has to replace. May only shrink.
const PULLS_BASELINE: &[(&str, usize)] = &[
];

/// `Arc<Engine>` in L4- and L5-owned production code. `l4_spawner.rs` and `l5_spawner.rs` are the handles themselves; the rest is workflows, which take an
/// `L4Spawner` once they move (S9b part 3).
const ARC_ENGINE_BASELINE: &[(&str, usize)] = &[
    ("github_intake.rs", 1),
    ("l4_spawner.rs", 1),
    ("bench/mod.rs", 1),
    ("l5_spawner.rs", 1),
    ("recovery_journal.rs", 1),
    ("scheduler.rs", 1),
];
/// `self.above.` and `self.authority.` call sites per file (`SuppliedFromAbove`: the quality block at dispatch and
/// functionary binding; `SpawnAuthority`: may this caller spawn a workflow node).
/// (page, metric, count) for every Wiring-owned file that reaches level state (`reach`), calls a level service
/// (`services`), reads `Facts::<People>` outside the routers (`people`) or calls `::provider(` directly (`providers`).
/// May only shrink; what each page composes is named in its entry in `OWNERS`.
const PAGE_BASELINE: &[(&str, &str, usize)] = &[
    ("access.rs", "reach", 22),
    ("access.rs", "services", 6),
    ("commands.rs", "people", 1),
    ("commands.rs", "providers", 2),
    ("commands.rs", "reach", 2),
    ("commands.rs", "services", 1),
    ("configuration.rs", "reach", 1),
    ("configuration.rs", "services", 1),
    ("doctor.rs", "reach", 2),
    ("engine.rs", "reach", 22),
    ("engine.rs", "services", 9),
    ("environments/promotion.rs", "reach", 6),
    ("environments/promotion.rs", "services", 3),
    ("environments/recovery.rs", "reach", 3),
    ("environments/recovery.rs", "services", 2),
    ("environments/report.rs", "people", 4),
    ("environments/report.rs", "reach", 3),
    ("environments/report.rs", "services", 1),
    ("facts/checks.rs", "providers", 8),
    ("facts/l1.rs", "reach", 3),
    ("facts/l1.rs", "services", 1),
    ("facts/l2.rs", "reach", 1),
    ("facts/l3.rs", "reach", 4),
    ("facts/l3.rs", "services", 1),
    ("facts/l4.rs", "reach", 10),
    ("facts/l5.rs", "providers", 1),
    ("facts/l5.rs", "reach", 2),
    ("facts/mod.rs", "providers", 2),
    ("facts/mod.rs", "services", 4),
    ("host_power/page.rs", "reach", 4),
    ("l2_pages.rs", "reach", 5),
    ("l2_pages.rs", "services", 2),
    ("main.rs", "services", 5),
    ("metrics.rs", "services", 2),
    ("occupancy_page.rs", "reach", 5),
    ("occupancy_page.rs", "services", 1),
    ("github_deployments.rs", "reach", 5),
    ("operations_report.rs", "reach", 8),
    ("production.rs", "people", 1),
    ("quality/mod.rs", "reach", 3),
    ("renewals/mod.rs", "reach", 8),
    ("renewals/mod.rs", "services", 1),
    ("runtime_events.rs", "reach", 3),
    ("runtime_events.rs", "services", 3),
    ("scope_views.rs", "reach", 6),
    ("signposts.rs", "people", 1),
    ("signposts.rs", "services", 1),
    ("site.rs", "reach", 3),
    ("supplied.rs", "providers", 3),
];
const ABOVE_BASELINE: &[(&str, usize)] = &[
    ("l4_service.rs", 1),
    ("run_start.rs", 1),
    ("verification.rs", 1),
];
const IMPERSONATION_BASELINE: &[(&str, usize)] = &[];
/// Direct `Port::provider` calls from level code, which reach a producer without the `Below` bound. Empty since S10
/// part 4 moved the last ones (L4 reading L5-produced workflow facts) behind `SuppliedFromAbove` and a page.
const DIRECT_PROVIDER_BASELINE: &[(&str, usize)] = &[];

/// Files that name `Facts::<People>` today (count). May only shrink.
const PEOPLE_BASELINE: &[(&str, usize)] = &[
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

/// Production code only: `#[cfg(test)] mod x { .. }` removed wherever it sits in
/// the file (a few files keep their tests before the code they test), and
/// comment lines dropped.
fn production_code(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut kept = String::new();
    let mut from = 0;
    while let Some(at) = source[from..].find("#[cfg(test)]") {
        let at = from + at;
        let after = at + "#[cfg(test)]".len();
        let rest = source[after..].trim_start();
        let is_mod = ["mod ", "pub mod ", "pub(crate) mod "].iter().any(|p| rest.starts_with(p));
        let open = source[after..].find('{').map(|i| after + i);
        let Some(open) = open.filter(|_| is_mod) else {
            kept.push_str(&source[from..after]);
            from = after;
            continue;
        };
        kept.push_str(&source[from..at]);
        // Skip to the matching brace, passing over strings, chars and comments.
        let mut depth = 0usize;
        let mut i = open;
        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    // raw string `r#"..."#`: count the hashes before the quote
                    let hashes = source[..i].bytes().rev().take_while(|b| *b == b'#').count();
                    let raw = source[..i - hashes].ends_with('r');
                    i += 1;
                    while i < bytes.len() {
                        if raw {
                            if bytes[i] == b'"' && source[i + 1..].bytes().take(hashes).all(|b| b == b'#') {
                                i += hashes;
                                break;
                            }
                        } else if bytes[i] == b'\\' {
                            i += 1;
                        } else if bytes[i] == b'"' {
                            break;
                        }
                        i += 1;
                    }
                }
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                    continue;
                }
                b'\'' => {
                    // a char literal `'x'` / `'\n'`; a lifetime has no closing quote nearby
                    if bytes.get(i + 2) == Some(&b'\'') {
                        i += 2;
                    } else if bytes.get(i + 1) == Some(&b'\\') && bytes.get(i + 3) == Some(&b'\'') {
                        i += 3;
                    }
                }
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        from = (i + 1).min(source.len());
    }
    kept.push_str(&source[from..]);
    kept.lines()
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

/// Reads the token that follows `at` after skipping whitespace; `None` at the end of the text.
fn next_token(code: &str, at: usize) -> Option<(&str, usize)> {
    let rest = &code[at..];
    let trimmed = rest.trim_start();
    let start = at + (rest.len() - trimmed.len());
    let first = trimmed.chars().next()?;
    let len = if first.is_alphanumeric() || first == '_' {
        trimmed.chars().take_while(|c| c.is_alphanumeric() || *c == '_').map(char::len_utf8).sum()
    } else {
        first.len_utf8()
    };
    Some((&code[start..start + len], start + len))
}

/// Every place `core` is used as a field of `self` (or of another `core`): `(position after "core", token after the
/// following dot)`. Whitespace between the tokens is tolerated, so a call split over lines still counts.
fn handle_fields(code: &str, handle: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (at, _) in code.match_indices("self") {
        let before_ok = !code[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
        if !before_ok {
            continue;
        }
        let Some((dot, p)) = next_token(code, at + 4) else { continue };
        if dot != "." {
            continue;
        }
        let Some((name, p)) = next_token(code, p) else { continue };
        if name != handle {
            continue;
        }
        let Some((dot, p)) = next_token(code, p) else { continue };
        if dot != "." {
            continue;
        }
        if let Some((field, _)) = next_token(code, p) {
            found.push(field.to_string());
        }
    }
    found
}

fn core_fields(code: &str) -> Vec<String> {
    handle_fields(code, "core")
}

/// `self.core.` call sites: the transitional `L4Service::core` handle on `Engine` (S9a).
fn core_calls(code: &str) -> usize {
    core_fields(code).len()
}

/// `self.above.` call sites: what L4 asks of the levels above, through `SuppliedFromAbove` (S10 part 4). The trait is
/// the whole list; each call site is counted so a new one is a visible decision.
fn above_calls(code: &str) -> usize {
    handle_fields(code, "above").len() + handle_fields(code, "authority").len()
}

/// Every `self.core` in production code, with or without a field after it (`foo(self.core)` counts). The transitional
/// `L4Service::core` handle is gone (S12), and this is what keeps it gone.
fn core_mentions(code: &str) -> usize {
    let mut found = 0;
    for (at, _) in code.match_indices("self") {
        let before_ok = !code[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_');
        if !before_ok {
            continue;
        }
        let Some((dot, p)) = next_token(code, at + 4) else { continue };
        if dot != "." {
            continue;
        }
        if let Some(("core", _)) = next_token(code, p) {
            found += 1;
        }
    }
    found
}

/// `core.l1`..`core.l6` and `core.shared`: the handle must never be a way to reach a level's state or the shared
/// group, which is exactly what the reach ratchet exists to count.
fn core_state_reaches(code: &str) -> usize {
    core_fields(code)
        .iter()
        .filter(|f| matches!(f.as_str(), "l1" | "l2" | "l3" | "l4" | "l5" | "l6" | "shared"))
        .count()
}

/// Fact reads that bypass the `Below` gate (S9b guard). Two shapes, both only in level-owned code (pages and wiring
/// are exempt; S12's page ratchet covers them):
/// - impersonation: `Facts::<Lx>::new(` where `Lx` is not the file's own level, so a level reads as another one;
/// - a direct `...::provider(` call (`XFact::provider(engine)`, `<X as Port>::provider(..)`), which reaches a producer
///   without the producer-below bound that `Wiring<L>::provider` carries.
fn impersonated_readers(code: &str, own_level: &str) -> usize {
    let squeezed: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let mut count = 0;
    for (at, _) in squeezed.match_indices("Facts::<") {
        let rest = &squeezed[at + "Facts::<".len()..];
        let Some(end) = rest.find(">::new(") else { continue };
        let level = rest[..end].rsplit("::").next().unwrap_or("");
        let is_level = matches!(level, "L1" | "L2" | "L3" | "L4" | "L5" | "L6");
        if is_level && level != own_level {
            count += 1;
        }
    }
    count
}

fn direct_provider_calls(code: &str) -> usize {
    let squeezed: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    squeezed.matches("::provider(").count() + squeezed.matches("::provider::<").count()
}

/// `Arc<Engine>` in a level file's production code. L4 code reaches the engine's `Arc` only through `L4Spawner`
/// (`l4_spawner.rs`), so the type may appear nowhere else in L4-owned files (S9b).
fn arc_engine_mentions(code: &str) -> usize {
    let squeezed: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    squeezed.matches("Arc<Engine>").count() + squeezed.matches("Arc<crate::engine::Engine>").count()
}

/// `wiring.record_status` / `wiring.record_gone`: L3 pushing a session's liveness into L4's record through
/// `Wiring`. The one upward call left behind a wiring method (S9 meant to replace it by L4 reading L3), so it is
/// counted where it is made, outside L4, and may only shrink.
fn liveness_bridge_calls(code: &str) -> usize {
    let squeezed: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    squeezed.matches("wiring.record_status(").count() + squeezed.matches("wiring.record_gone(").count()
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
    core: BTreeMap<String, usize>,
    core_state: BTreeMap<String, usize>,
    impersonation: BTreeMap<String, usize>,
    direct_provider: BTreeMap<String, usize>,
    arc_engine: BTreeMap<String, usize>,
    above: BTreeMap<String, usize>,
    bridge: BTreeMap<String, usize>,
    pages: BTreeMap<(String, &'static str), usize>,
}

fn scan() -> Scan {
    let root = src_root();
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    files.sort();
    let mut result = Scan { unplaced: Vec::new(), reach: BTreeMap::new(), people: BTreeMap::new(), pulls: BTreeMap::new(), core: BTreeMap::new(), core_state: BTreeMap::new(), impersonation: BTreeMap::new(), direct_provider: BTreeMap::new(), arc_engine: BTreeMap::new(), above: BTreeMap::new(), bridge: BTreeMap::new(), pages: BTreeMap::new() };
    for file in files {
        let relative = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if is_test_file(&relative) {
            continue;
        }
        let Some(owner) = owner_of(&relative) else {
            result.unplaced.push(relative);
            continue;
        };
        let code = production_code(&std::fs::read_to_string(&file).unwrap());
        let state_reaches = core_state_reaches(&code);
        if state_reaches > 0 {
            result.core_state.insert(relative.clone(), state_reaches);
        }
        let above = above_calls(&code);
        if above > 0 {
            result.above.insert(relative.clone(), above);
        }
        let core = core_calls(&code);
        if core > 0 {
            result.core.insert(relative.clone(), core);
        }
        let Some(own_group) = level_group(owner) else {
            // A page (wiring, the entry point, composition over several levels) may reach any group today; what it
            // does is counted per page and may only shrink (S12).
            let reach: usize = group_reach(&code).values().sum();
            let services: usize = SERVICES.iter().map(|service| code.matches(service.accessor).count()).sum();
            let people = if relative.starts_with("router/") { 0 } else { people_reach(&code) };
            let providers = direct_provider_calls(&code);
            for (metric, count) in [("reach", reach), ("services", services), ("people", people), ("providers", providers)] {
                if count > 0 {
                    result.pages.insert((relative.clone(), metric), count);
                }
            }
            continue;
        };
        let impersonated = impersonated_readers(&code, &own_group.to_uppercase());
        if impersonated > 0 {
            result.impersonation.insert(relative.clone(), impersonated);
        }
        let bridge = liveness_bridge_calls(&code);
        if bridge > 0 {
            result.bridge.insert(relative.clone(), bridge);
        }
        if own_group == "l4" || own_group == "l5" {
            let arcs = arc_engine_mentions(&code);
            if arcs > 0 {
                result.arc_engine.insert(relative.clone(), arcs);
            }
        }
        let direct = direct_provider_calls(&code);
        if direct > 0 {
            result.direct_provider.insert(relative.clone(), direct);
        }
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
    out.push_str("];\nconst IMPERSONATION_BASELINE: &[(&str, usize)] = &[\n");
    for (file, count) in &scan.impersonation {
        out.push_str(&format!("    ({file:?}, {count}),\n"));
    }
    out.push_str("];\nconst DIRECT_PROVIDER_BASELINE: &[(&str, usize)] = &[\n");
    for (file, count) in &scan.direct_provider {
        out.push_str(&format!("    ({file:?}, {count}),\n"));
    }
    out.push_str("];\nconst ARC_ENGINE_BASELINE: &[(&str, usize)] = &[\n");
    for (file, count) in &scan.arc_engine {
        out.push_str(&format!("    ({file:?}, {count}),\n"));
    }
    out.push_str("];\nconst PAGE_BASELINE: &[(&str, &str, usize)] = &[\n");
    for ((file, metric), count) in &scan.pages {
        out.push_str(&format!("    ({file:?}, {metric:?}, {count}),\n"));
    }
    out.push_str("];\nconst ABOVE_BASELINE: &[(&str, usize)] = &[\n");
    for (file, count) in &scan.above {
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

/// `L4Service::core`, the transitional handle on `Engine`, no longer exists, and nothing may bring it back: no
/// `self.core` anywhere in production code, and no `core` field on the L4 service.
#[test]
fn l4_service_has_no_core_handle() {
    let root = src_root();
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let mut found = Vec::new();
    for file in files {
        let relative = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if is_test_file(&relative) {
            continue;
        }
        let code = production_code(&std::fs::read_to_string(&file).unwrap());
        let mentions = core_mentions(&code);
        if mentions > 0 {
            found.push(format!("{relative}: {mentions} `self.core`"));
        }
        if relative == "l4_service.rs" {
            let squeezed: String = code.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(!squeezed.contains("core:&'a"), "L4Service must not have a `core` field");
        }
    }
    assert!(found.is_empty(), "the `core` handle is gone; do not bring it back: {found:#?}");
}

#[test]
fn the_core_scanner_counts_split_calls_and_catches_state_reaches() {
    assert_eq!(core_calls("self.core.a(); self\n    .core\n    .b(); other_core.c(); itself.core.d();"), 2);
    assert_eq!(core_mentions("f(self.core); self . core . g(); itself.core; self.corex; self.core_handle"), 2);
    assert_eq!(core_state_reaches("self.core.l3.x; self.core\n.shared.y; self.core.l3_service(); self.core.shared_thing()"), 2);
    assert_eq!(core_state_reaches("score.l3.x; self.core.state()"), 0);
}

/// `Intent` is the one way a level reads what L6 authored (S9b design step). It may only call pure functions over the
/// authored files and the snapshot: it must never name an L6 service, an L6 state group or a store type, or it would
/// become a back door around the ladder. Consumers that need L6 computation over runtime state are inputs of the
/// downward command instead.
#[test]
fn intent_stays_pure_and_never_reaches_an_l6_service_state_or_store() {
    let code = production_code(&std::fs::read_to_string(src_root().join("intent.rs")).unwrap());
    let squeezed: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    for forbidden in ["l6_service", "L6Service", ".l6.", ".l6;", "Store", "self.engine", "Engine", "state."] {
        assert!(!squeezed.contains(forbidden), "intent.rs must stay pure; it names `{forbidden}`");
    }
}

fn only_shrinks(label: &str, found: &BTreeMap<String, usize>, baseline: &[(&str, usize)], scan: &Scan) {
    let baseline: BTreeMap<&str, usize> = baseline.iter().copied().collect();
    let mut bad = Vec::new();
    for (file, count) in found {
        match baseline.get(file.as_str()) {
            None => bad.push(format!("{file}: {count} {label}, new")),
            Some(allowed) if count > allowed => bad.push(format!("{file}: {count} {label} > {allowed}")),
            Some(allowed) if count < allowed => bad.push(format!("{file}: {count} {label} < baseline {allowed}: lower it")),
            _ => {}
        }
    }
    for file in baseline.keys() {
        if !found.contains_key(*file) {
            bad.push(format!("{file}: no longer has {label}, remove it from the baseline"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}\ncurrent state:\n{}", describe(scan));
}

#[test]
fn a_level_never_reads_facts_as_another_level_and_only_shrinks() {
    let scan = scan();
    only_shrinks("`Facts::<Lx>::new` readers of another level", &scan.impersonation, IMPERSONATION_BASELINE, &scan);
}

#[test]
fn l4_code_holds_an_arc_engine_only_through_the_spawner_and_the_rest_only_shrinks() {
    let scan = scan();
    only_shrinks("`Arc<Engine>` mentions", &scan.arc_engine, ARC_ENGINE_BASELINE, &scan);
    assert!(
        !scan.arc_engine.contains_key("verification.rs")
            && !scan.arc_engine.contains_key("intake.rs")
            && !scan.arc_engine.contains_key("bench/engine.rs"),
        "verification and intake take an `L4Spawner`, bench an `L5Spawner`, never an `Arc<Engine>`"
    );
}

/// The pages (Wiring-owned files: composition over several levels, wiring, the entry point) are the largest area the
/// level ratchets do not see. Each page's reach into level state, calls into level services, People reads and raw
/// provider calls are counted here and may only shrink (S12).
#[test]
fn pages_only_shrink() {
    let scan = scan();
    let baseline: BTreeMap<(String, &str), usize> =
        PAGE_BASELINE.iter().map(|(file, metric, count)| ((file.to_string(), *metric), *count)).collect();
    let mut bad = Vec::new();
    for (key, count) in &scan.pages {
        match baseline.get(&(key.0.clone(), key.1)) {
            None => bad.push(format!("{}: {count} {} on a page, new", key.0, key.1)),
            Some(allowed) if count > allowed => bad.push(format!("{}: {count} {} > {allowed}", key.0, key.1)),
            Some(allowed) if count < allowed => {
                bad.push(format!("{}: {count} {} < baseline {allowed}: lower PAGE_BASELINE", key.0, key.1))
            }
            _ => {}
        }
    }
    for (file, metric) in baseline.keys() {
        if !scan.pages.contains_key(&(file.clone(), metric)) {
            bad.push(format!("{file}: no longer has {metric}: remove it from PAGE_BASELINE"));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}\ncurrent state:\n{}", describe(&scan));
}

/// L3 no longer pushes anything into L4: standing agents' liveness is L3's observation log, which L4 reads as a fact
/// (`StandingAgentObservationsFact`). The `Wiring` bridge that carried it upward is gone and may not come back.
#[test]
fn l3_pushes_no_liveness_into_l4_through_wiring() {
    let scan = scan();
    assert!(scan.bridge.is_empty(), "`wiring.record_status`/`record_gone` is gone; do not bring it back: {:?}", scan.bridge);
}

#[test]
fn what_l4_asks_of_the_levels_above_only_shrinks() {
    let scan = scan();
    only_shrinks("`self.above.` capability calls", &scan.above, ABOVE_BASELINE, &scan);
}

#[test]
fn direct_provider_calls_in_level_code_only_shrink() {
    let scan = scan();
    only_shrinks("direct `::provider(` calls", &scan.direct_provider, DIRECT_PROVIDER_BASELINE, &scan);
}

#[test]
fn the_fact_guard_scanners_see_impersonation_and_direct_providers() {
    assert_eq!(impersonated_readers("Facts::<L6>::new(self).get(); crate::facts::Facts::<factory_kernel::L5>::new(\n x)", "L4"), 2);
    assert_eq!(impersonated_readers("Facts::<L4>::new(self); Facts::<People>::new(self)", "L4"), 0);
    assert_eq!(direct_provider_calls("XFact::provider(engine); <Y as Port>::provider(&e); self.wiring.provider::<Z>()"), 2);
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
    // A test module in the middle of a file hides only itself, braces in strings included.
    let code = production_code(
        "fn a(&self) { self.l1.x; }\n#[cfg(test)]\nmod tests { fn t() { let s = \"}{\"; let c = '}'; self.l2.y; } }\nfn b(&self) { self.l3.z; }\n",
    );
    let reach = group_reach(&code);
    assert_eq!((reach.get("l1"), reach.get("l2"), reach.get("l3")), (Some(&1), None, Some(&1)));
}
