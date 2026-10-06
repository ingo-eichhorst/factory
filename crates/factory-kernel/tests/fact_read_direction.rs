//! Check the actual public read API from an external crate, not a surrogate
//! trait assertion. Each invalid edge must fail for the Below bound itself.
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("factory-fact-direction-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        let kernel = Path::new(env!("CARGO_MANIFEST_DIR")).to_str().unwrap();
        let manifest = format!(
            "[package]\nname = \"fact-read-direction-probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\
             [workspace]\n[dependencies]\nfactory-kernel = {{ path = {kernel:?} }}\n\
             serde = {{ version = \"1\", features = [\"derive\"] }}\n"
        );
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        // Use the dependencies already resolved for the outer workspace;
        // an offline probe must not pick a newer, uncached transitive version.
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap();
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
        assert_eq!(output.status.success(), allowed, "source:\n{source}\ncompiler:\n{errors}");
        if !allowed {
            assert!(errors.contains(reason), "wrong rejection reason ({reason}):\n{errors}");
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        // Only this test's uniquely created scratch directory is removed.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn read_source(producer: &str, reader: &str) -> String {
    format!(
        "#![allow(dead_code)]\nuse factory_kernel::*;\n\
         #[derive(serde::Serialize, serde::Deserialize)] struct Sample;\n\
         impl Fact for Sample {{ type Producer = {producer}; }}\n\
         async fn read<P: Provide<Sample, Query = ()>>(provider: &P) {{\n\
             let _ = Facts::<{reader}>::new().get::<Sample, P>(provider, &()).await;\n\
         }}\n"
    )
}

#[test]
fn every_level_pair_and_people_reader_are_checked_by_the_real_get_method() {
    let fixture = Fixture::new();
    let mut upward = 0;
    let mut forbidden = 0;
    for producer in 1..=6 {
        for reader in 1..=6 {
            let allowed = producer < reader;
            fixture.check(&read_source(&format!("L{producer}"), &format!("L{reader}")), allowed, "Below<");
            if allowed {
                upward += 1;
            } else {
                forbidden += 1;
            }
        }
        fixture.check(&read_source(&format!("L{producer}"), "People"), true, "");
    }
    assert_eq!((upward, forbidden), (15, 21));
    fixture.check(
        "use factory_kernel::*; struct Extra; impl Level for Extra {} impl Below<L6> for Extra {}",
        false,
        "sealed",
    );
    fixture.check(&read_source("People", "L6"), false, "Level");
}

#[test]
fn live_check_result_fact_allows_direction_and_people_but_rejects_process_and_same_level() {
    let fixture = Fixture::new();
    for (reader, allowed) in [("L6", true), ("People", true), ("L4", false), ("L5", false)] {
        let source = format!("use factory_kernel::*; async fn read<P: Provide<CheckEvaluationFact, Query = ()>>(provider: &P) {{ let _ = Facts::<{reader}>::new().get::<CheckEvaluationFact, P>(provider, &()).await; }}");
        fixture.check(&source, allowed, "Below<");
    }
}

fn wired_source(producer: &str, reader: &str) -> String {
    format!(
        "#![allow(dead_code)]\nuse factory_kernel::*;\n\
         #[derive(serde::Serialize, serde::Deserialize)] struct Sample;\n\
         impl Fact for Sample {{ type Producer = {producer}; }}\n\
         struct Host;\n\
         fn reach(host: &Host) {{ let _ = Wired::<{reader}, Host>::new(host).reach::<Sample>(); }}\n"
    )
}

/// The host a level service builds providers from is reachable only for a fact the
/// service's level may read: an L2 handle cannot reach an L4 fact's provider.
#[test]
fn a_level_bound_wiring_reaches_only_producers_below_its_level() {
    let fixture = Fixture::new();
    let mut allowed_count = 0;
    let mut forbidden = 0;
    for producer in 1..=6 {
        for reader in 1..=6 {
            let allowed = producer < reader;
            fixture.check(&wired_source(&format!("L{producer}"), &format!("L{reader}")), allowed, "Below<");
            if allowed {
                allowed_count += 1;
            } else {
                forbidden += 1;
            }
        }
    }
    assert_eq!((allowed_count, forbidden), (15, 21));
    // The named case: an L2 service cannot read an L4 fact through its wiring.
    fixture.check(&wired_source("L4", "L2"), false, "Below<");
    fixture.check(&wired_source("L1", "L2"), true, "");
}
