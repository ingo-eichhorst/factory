//! Check the actual public command API from an external crate, not a surrogate
//! trait assertion. Each invalid edge must fail for the DirectlyBelow bound itself.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// The tests in this file run as threads of one process, and macOS clocks only
/// resolve microseconds, so pid and time alone can name two fixtures the same.
/// The counter keeps them apart; pid and time keep other processes apart.
/// `scratch_root` takes the clock reading so a test can hold it fixed.
static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch_root(nonce: u128) -> PathBuf {
    std::env::temp_dir().join(format!(
        "factory-command-direction-{}-{nonce}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = scratch_root(nonce);
        std::fs::create_dir_all(root.join("src")).unwrap();
        let kernel = Path::new(env!("CARGO_MANIFEST_DIR")).to_str().unwrap();
        let manifest = format!(
            "[package]\nname = \"command-direction-probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\
             [workspace]\n[dependencies]\nfactory-kernel = {{ path = {kernel:?} }}\n\
             serde = {{ version = \"1\", features = [\"derive\"] }}\n"
        );
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        // Use the dependencies already resolved for the outer workspace;
        // an offline probe must not pick a newer, uncached transitive version.
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        std::fs::copy(workspace.join("Cargo.lock"), root.join("Cargo.lock")).unwrap();
        Self(root)
    }

    fn check(&self, source: &str, allowed: bool, reason: &str) {
        std::fs::write(self.0.join("src/lib.rs"), source).unwrap();
        // A private target avoids the outer workspace's Cargo lock. Offline
        // mode ensures this compiler proof never needs a service or network.
        let output = Command::new(env!("CARGO"))
            .args(["check", "--offline", "--quiet", "--manifest-path"])
            .arg(self.0.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", self.0.join("target"))
            .output()
            .unwrap();
        let errors = String::from_utf8_lossy(&output.stderr);
        assert_eq!(
            output.status.success(),
            allowed,
            "source:\n{source}\ncompiler:\n{errors}"
        );
        if !allowed {
            assert!(
                errors.contains(reason),
                "wrong rejection reason ({reason}):\n{errors}"
            );
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only this test's uniquely created scratch directory is removed.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn scratch_roots_are_unique_within_the_process() {
    // Every fixture started in the same microsecond still gets its own directory.
    let roots: HashSet<PathBuf> = (0..100).map(|_| scratch_root(0)).collect();
    assert_eq!(roots.len(), 100);
}

fn command_source(producer: &str, caller: &str) -> String {
    format!("#![allow(dead_code)]\nuse factory_kernel::*;\nstruct Port;\nimpl CommandPort for Port {{type Level={producer};}}\nfn call() {{let p=Commands::<{caller},_>::new(Port); let _=p.port();}}\n")
}

#[test]
fn exactly_five_adjacent_command_edges_compile_through_the_real_wrapper() {
    let fixture = Fixture::new();
    let mut allowed = 0;
    let mut forbidden = 0;
    for producer in 1..=6 {
        for caller in 1..=6 {
            let adjacent = caller == producer + 1;
            fixture.check(
                &command_source(&format!("L{producer}"), &format!("L{caller}")),
                adjacent,
                "DirectlyBelow<",
            );
            if adjacent {
                allowed += 1;
            } else {
                forbidden += 1;
            }
        }
        fixture.check(
            &command_source(&format!("L{producer}"), "People"),
            false,
            "Level",
        );
    }
    assert_eq!((allowed, forbidden), (5, 31));
    fixture.check("use factory_kernel::*; struct Extra; impl Level for Extra {} impl DirectlyBelow<L6> for Extra {}",false,"sealed");
    fixture.check(&command_source("People", "L6"), false, "Level");
}

/// Named, because dispatch is where it would be tempting: Process may command Agent
/// (`Commands<L4, L3Port>`), but cannot name `Commands<L4, L2Port>` or `Commands<L4, L1Port>`:
/// provisioning an environment goes through Agent, which asks Environment.
#[test]
fn process_commands_agent_but_cannot_name_a_command_port_two_levels_down() {
    let fixture = Fixture::new();
    fixture.check(&command_source("L3", "L4"), true, "");
    fixture.check(&command_source("L2", "L4"), false, "DirectlyBelow<");
    fixture.check(&command_source("L1", "L4"), false, "DirectlyBelow<");
    // And Agent may command Environment, not Infrastructure.
    fixture.check(&command_source("L2", "L3"), true, "");
    fixture.check(&command_source("L1", "L3"), false, "DirectlyBelow<");
}
