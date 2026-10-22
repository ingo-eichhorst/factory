//! The `factory` binary builds and runs.
//!
//! Small as it is, this file is also why the daemon's end-to-end suites
//! (`crates/factory-daemon/tests/{workflow_shell_e2e,pages_e2e}.rs`) find a
//! current CLI under `cargo test --workspace`: cargo only builds a package's
//! binary as `target/<profile>/factory` -- and rebuilds it when its source
//! changes -- when that package has an integration test. Without one, the
//! workspace test run builds `factory` only as a unit-test harness, and the
//! suites would run against no CLI or a stale one.

use std::process::Command;

#[test]
fn the_cli_reports_its_version_without_a_daemon() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_factory"));
    command.arg("--version");
    // `--version` is answered while parsing, before any daemon is located;
    // removing these keeps it that way should that ever change.
    for name in [
        "FACTORY_SOCKET",
        "FACTORY_URL",
        "FACTORY_ROOT",
        "FACTORY_TOKEN",
    ] {
        command.env_remove(name);
    }
    let output = command.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{output:?}");
    assert!(stdout.starts_with("factory "), "{stdout}");
}
