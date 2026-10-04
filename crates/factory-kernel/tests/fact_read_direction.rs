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
