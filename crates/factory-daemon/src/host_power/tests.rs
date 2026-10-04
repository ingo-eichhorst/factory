//! `#260` against a pretend host: the parsers on `pmset`'s real output, the
//! exact commands built, and the read/set flow end to end through
//! `Engine::handle_request` -- never a real `pmset` or `sudo`.

use super::testing::{FakeHost, MAC_CAP};
use super::*;
use crate::engine::Engine;
use factory_core::config::{Config, Factory};
use factory_core::protocol::{Payload, Request, Response};
use factory_plugins::{Registry, SqliteStore};
use std::path::PathBuf;

/// `pmset -g custom` on the issue's host (`Mac17,7`, macOS 26.6.1), as read.
const HOST_CUSTOM: &str = "Battery Power:
 Sleep On Power Button 1
 powermode            0
 standby              1
 ttyskeepawake        1
 hibernatemode        3
 powernap             1
 hibernatefile        /var/vm/sleepimage
 displaysleep         2
 womp                 0
 networkoversleep     0
 sleep                1
 lessbright           1
 tcpkeepalive         1
 disksleep            10
AC Power:
 Sleep On Power Button 1
 powermode            0
 standby              0
 ttyskeepawake        1
 hibernatemode        3
 powernap             0
 hibernatefile        /var/vm/sleepimage
 displaysleep         10
 womp                 1
 networkoversleep     0
 sleep                0
 tcpkeepalive         1
 disksleep            10
 SleepServices        0
";

/// `pmset -g cap` on the same host.
const HOST_CAP: &str = "Capabilities for AC Power:
 displaysleep
 disksleep
 sleep
 womp
 standby
 powernap
 ttyskeepawake
 hibernatemode
 hibernatefile
 tcpkeepalive
 lowpowermode
 highpowermode
";

#[test]
fn the_hosts_own_custom_output_reads_automatic_on_both_sources() {
    let c = parse_custom(HOST_CUSTOM);
    assert_eq!(c.ac, Some(PowerMode::Automatic));
    assert_eq!(c.battery, Some(PowerMode::Automatic));
    assert!(c.notes.is_empty());
}

#[test]
fn each_source_is_read_on_its_own_and_a_desktop_has_no_battery() {
    let mixed = "Battery Power:\n powermode 1\nAC Power:\n powermode 2\n";
    let c = parse_custom(mixed);
    assert_eq!(c.battery, Some(PowerMode::EnergySaving));
    assert_eq!(c.ac, Some(PowerMode::HighPerformance));

    let desktop = "AC Power:\n Sleep On Power Button 1\n powermode            2\n";
    let c = parse_custom(desktop);
    assert_eq!(c.ac, Some(PowerMode::HighPerformance));
    assert_eq!(c.battery, None);

    // A UPS section's settings are not mistaken for AC's.
    let ups = "AC Power:\n powermode 0\nUPS Power:\n powermode 2\n";
    assert_eq!(parse_custom(ups).ac, Some(PowerMode::Automatic));
}

#[test]
fn an_older_macos_lowpowermode_is_read_and_an_unknown_value_is_said_not_guessed() {
    let old = "Battery Power:\n lowpowermode 1\nAC Power:\n lowpowermode 0\n";
    let c = parse_custom(old);
    assert_eq!(c.battery, Some(PowerMode::EnergySaving));
    assert_eq!(c.ac, Some(PowerMode::Automatic));

    let future = "AC Power:\n powermode 3\n";
    let c = parse_custom(future);
    assert_eq!(c.ac, None);
    assert_eq!(c.notes.len(), 1, "{:?}", c.notes);
    assert!(c.notes[0].contains("powermode 3"));
}

#[test]
fn capabilities_decide_which_modes_are_offered() {
    assert_eq!(parse_cap(HOST_CAP), PowerMode::ALL.to_vec());
    // An Air-class Mac: Low Power, no High Power.
    assert_eq!(
        parse_cap("Capabilities for AC Power:\n sleep\n lowpowermode\n"),
        vec![PowerMode::Automatic, PowerMode::EnergySaving]
    );
    assert!(parse_cap("Capabilities for AC Power:\n sleep\n womp\n").is_empty());
    // A substring is not a capability.
    assert!(parse_cap(" nolowpowermodehere\n").is_empty());
}

#[test]
fn every_mode_maps_to_its_pmset_number_and_the_commands_are_exact() {
    assert_eq!(pmset_value(PowerMode::Automatic), "0");
    assert_eq!(pmset_value(PowerMode::EnergySaving), "1");
    assert_eq!(pmset_value(PowerMode::HighPerformance), "2");
    assert_eq!(
        sudo_set_args(PowerMode::HighPerformance),
        [
            "-n",
            "-k",
            "-u",
            "root",
            "--",
            "/usr/bin/pmset",
            "-a",
            "powermode",
            "2"
        ]
    );
    assert_eq!(
        sudo_probe_args(PowerMode::EnergySaving),
        [
            "-n",
            "-k",
            "-l",
            "-l",
            "-u",
            "root",
            "--",
            "/usr/bin/pmset",
            "-a",
            "powermode",
            "1"
        ]
    );
}

#[test]
fn the_sudoers_rule_names_exactly_the_three_commands_for_the_daemons_user() {
    let rule = sudoers_rule("factory");
    assert_eq!(
        rule.rule,
        "factory ALL=(root) NOPASSWD: /usr/bin/pmset -a powermode 0, /usr/bin/pmset -a powermode 1, \
         /usr/bin/pmset -a powermode 2"
    );
    assert_eq!(rule.path, "/etc/sudoers.d/factory-pmset");
    // Checked before it is installed, installed root-owned and read-only.
    let check = rule.install.find("visudo -cf").unwrap();
    let install = rule
        .install
        .find("install -m 0440 -o root -g wheel")
        .unwrap();
    assert!(check < install, "{}", rule.install);
    assert!(rule.install.contains(&format!("'{}'", rule.rule)));
    assert!(
        rule.install
            .ends_with("/etc/sudoers.d/factory-pmset; rm -f \"$f\"")
    );
    assert_eq!(rule.check, "sudo visudo -c");
}

#[test]
fn json_names_are_the_three_closed_values() {
    for (mode, name) in [
        (PowerMode::Automatic, "automatic"),
        (PowerMode::HighPerformance, "high_performance"),
        (PowerMode::EnergySaving, "energy_saving"),
    ] {
        assert_eq!(serde_json::to_value(mode).unwrap(), name);
        assert_eq!(mode.as_str(), name);
        assert_eq!(name.parse::<PowerMode>().unwrap(), mode);
        assert_eq!(name.replace('_', "-").parse::<PowerMode>().unwrap(), mode);
    }
    for bad in ["3", "auto; rm", "", "Automatic ", "high"] {
        assert!(bad.parse::<PowerMode>().is_err(), "{bad:?}");
        assert!(
            serde_json::from_value::<PowerMode>(serde_json::json!(bad)).is_err(),
            "{bad:?}"
        );
    }
    assert!(serde_json::from_value::<PowerMode>(serde_json::json!(2)).is_err());
}

#[tokio::test]
async fn without_the_rule_the_mode_is_read_and_nothing_is_written() {
    let host = FakeHost::mac();
    let power = HostPower::with_runner(host.clone(), "factory");
    let report = power.read().await;
    assert!(report.applicable);
    assert_eq!(report.supported, PowerMode::ALL.to_vec());
    assert_eq!(report.ac, Some(PowerMode::Automatic));
    assert_eq!(report.battery, Some(PowerMode::Automatic));
    assert!(report.permitted.is_empty());
    assert!(!report.can_change);
    assert!(
        report.notes.is_empty(),
        "no rule is the ordinary state, not a note: {:?}",
        report.notes
    );
    assert!(
        report
            .sudoers
            .rule
            .starts_with("factory ALL=(root) NOPASSWD: ")
    );
    assert!(host.writes().is_empty(), "{:?}", host.calls());
    // Probed with `-l` for each mode, never run.
    let probes: Vec<_> = host.calls().into_iter().filter(|c| c[0] == SUDO).collect();
    assert_eq!(probes.len(), 3);
    assert!(probes.iter().all(|c| c[1..5] == ["-n", "-k", "-l", "-l"]));
}

#[tokio::test]
async fn setting_without_the_rule_is_refused_before_any_write() {
    let host = FakeHost::mac();
    let power = HostPower::with_runner(host.clone(), "factory");
    match power.set(PowerMode::HighPerformance).await {
        Err(FactoryError::BadRequest(why)) => {
            assert!(why.contains("/etc/sudoers.d/factory-pmset"), "{why}")
        }
        other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
    }
    assert!(host.writes().is_empty(), "{:?}", host.calls());
}

#[tokio::test]
async fn with_the_rule_each_mode_runs_its_own_command_and_reads_back() {
    let host = FakeHost::mac();
    host.install_rule();
    let power = HostPower::with_runner(host.clone(), "factory");
    assert!(power.read().await.can_change);
    for (mode, n) in [
        (PowerMode::HighPerformance, "2"),
        (PowerMode::EnergySaving, "1"),
        (PowerMode::Automatic, "0"),
    ] {
        let before = host.writes().len();
        let changed = power.set(mode).await.unwrap();
        let writes = host.writes();
        assert_eq!(writes.len(), before + 1);
        assert_eq!(
            writes.last().unwrap(),
            &vec![
                SUDO,
                "-n",
                "-k",
                "-u",
                "root",
                "--",
                PMSET,
                "-a",
                "powermode",
                n
            ]
        );
        assert_eq!(changed.after.ac, Some(mode));
        assert_eq!(changed.after.battery, Some(mode));
        let report = power.read().await;
        assert_eq!((report.ac, report.battery), (Some(mode), Some(mode)));
    }
}

#[tokio::test]
async fn a_mode_the_host_does_not_offer_is_refused_before_sudo_is_asked() {
    let host = std::sync::Arc::new(FakeHost {
        cap: "Capabilities for AC Power:\n sleep\n lowpowermode\n".into(),
        ac: std::sync::Mutex::new(Some("0")),
        battery: std::sync::Mutex::new(Some("0")),
        permitted: std::sync::Mutex::new(vec!["0", "1", "2"]),
        admin_group: super::testing::ADMIN_GROUP.to_string(),
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let power = HostPower::with_runner(host.clone(), "factory");
    let report = power.read().await;
    assert_eq!(
        report.supported,
        vec![PowerMode::Automatic, PowerMode::EnergySaving]
    );
    assert!(report.can_change, "every mode the host offers is permitted");
    let asked = host.calls().len();
    assert!(matches!(
        power.set(PowerMode::HighPerformance).await,
        Err(FactoryError::BadRequest(_))
    ));
    let after: Vec<_> = host.calls().into_iter().skip(asked).collect();
    assert!(
        after.iter().all(|c| c[0] == PMSET),
        "no sudo at all for an unoffered mode: {after:?}"
    );
}

#[tokio::test]
async fn a_host_with_no_energy_mode_offers_nothing_and_probes_nothing() {
    let host = std::sync::Arc::new(FakeHost {
        cap: "Capabilities for AC Power:\n sleep\n womp\n".into(),
        ac: std::sync::Mutex::new(None),
        battery: std::sync::Mutex::new(None),
        permitted: std::sync::Mutex::new(Vec::new()),
        admin_group: super::testing::ADMIN_GROUP.to_string(),
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let power = HostPower::with_runner(host.clone(), "factory");
    let report = power.read().await;
    assert!(report.applicable);
    assert!(report.supported.is_empty());
    assert!(!report.can_change);
    assert!(
        host.calls().iter().all(|c| c[0] != SUDO),
        "nothing offered, so sudo is never asked"
    );
}

#[tokio::test]
async fn no_runner_is_not_applicable_and_setting_is_refused() {
    let power = HostPower {
        runner: std::sync::RwLock::new(None),
        user: "factory".into(),
        set_lock: tokio::sync::Mutex::new(()),
    };
    let report = power.read().await;
    assert!(!report.applicable);
    assert!(report.supported.is_empty());
    assert!(!report.can_change);
    assert!(matches!(
        power.set(PowerMode::Automatic).await,
        Err(FactoryError::BadRequest(_))
    ));
}

#[test]
fn a_test_build_never_gets_the_real_runner() {
    assert!(
        HostPower::new().runner().is_none(),
        "cargo test must never run a real pmset or sudo"
    );
}

fn engine() -> Arc<Engine> {
    let config: Config = serde_yaml_ng::from_str(
        "instance:\n  id: i\n  name: test\nscope:\n  name: demo\n  path: .\nscopes:\n  - name: demo\n    path: .\n",
    )
    .unwrap();
    config.validate().unwrap();
    let factory = Factory {
        root: std::env::temp_dir()
            .join(format!("factory-host-power-test-{}", uuid::Uuid::new_v4())),
        config,
    };
    Arc::new(Engine::new(
        factory,
        Registry::with_builtins(),
        Arc::new(SqliteStore::in_memory().unwrap()),
        PathBuf::from("factory"),
        vec![],
    ))
}

fn report_of(response: Response) -> PowerModeReport {
    match response {
        Response::Ok {
            data: Payload::HostPowerMode { report },
        } => report,
        other => panic!("expected a power mode report, got {other:?}"),
    }
}

#[tokio::test]
async fn through_the_engine_a_change_is_journaled_with_who_from_and_to() {
    let engine = engine();
    let host = FakeHost::mac();
    engine.host_power.replace_runner(host.clone());

    let report = report_of(engine.handle_request(Request::HostPowerMode).await);
    assert!(!report.can_change);
    assert!(report.changes.is_empty());

    // Refused without the rule: an error, and no journal line.
    let refused = engine
        .handle_request(Request::HostPowerModeSet {
            mode: PowerMode::HighPerformance,
        })
        .await;
    assert!(
        matches!(&refused, Response::Error { code, .. } if code == "bad_request"),
        "{refused:?}"
    );
    assert!(host.writes().is_empty());

    host.install_rule();
    let report = report_of(
        engine
            .handle_request(Request::HostPowerModeSet {
                mode: PowerMode::HighPerformance,
            })
            .await,
    );
    assert_eq!(report.ac, Some(PowerMode::HighPerformance));
    assert_eq!(report.battery, Some(PowerMode::HighPerformance));
    assert_eq!(report.changes.len(), 1);
    let change = &report.changes[0];
    assert_eq!(change.by, "the owner");
    assert_eq!(change.from_ac, Some(PowerMode::Automatic));
    assert_eq!(change.from_battery, Some(PowerMode::Automatic));
    assert_eq!(change.to, PowerMode::HighPerformance);
    assert_eq!(
        change.message,
        "power mode: Automatic -> High performance by the owner"
    );

    // A later read still shows it.
    let report = report_of(engine.handle_request(Request::HostPowerMode).await);
    assert_eq!(report.changes.len(), 1);
}

#[test]
fn the_mac_cap_fixture_is_the_hosts_shape() {
    assert_eq!(parse_cap(MAC_CAP), parse_cap(HOST_CAP));
}

#[test]
fn listing_success_is_not_passwordless_permission() {
    let listing = |options: &str, matched: &str| {
        format!(
            "Sudoers entry: /etc/sudoers\n    RunAsUsers: root\n    Options: {options}\n    Commands:\n        ALL\n    Matched: {matched}\n"
        )
    };
    let mode = PowerMode::HighPerformance;
    let command = "/usr/bin/pmset -a powermode 2";
    assert!(passwordless_match(&listing("!authenticate", command), mode));
    assert!(passwordless_match(
        &listing("noexec, !authenticate, log_output", command),
        mode
    ));
    for text in [
        String::new(),
        command.to_string(),              // short listing / cached credentials
        listing("authenticate", command), // listpw=any, unrelated NOPASSWD
        listing("!authenticate, authenticate", command),
        listing("!authenticate", "/usr/bin/pmset -a powermode 3"),
        listing("!authenticate", "/usr/bin/pmset -a powermode 2 extra"),
        format!(
            "{}{}",
            listing("!authenticate", command),
            listing("authenticate", command)
        ),
        "Matching Defaults entries: !authenticate\n    Matched: /usr/bin/pmset -a powermode 2\n"
            .into(),
        "Options: !authenticate\nSudoers entry: /etc/sudoers\n    Matched: /usr/bin/pmset -a powermode 2\n".into(),
    ] {
        assert!(!passwordless_match(&text, mode), "must fail closed: {text}");
    }
}

/// Overrides only the read-only command. All writes still go through the
/// recording fake, so the tests can prove how many were attempted.
struct ReadbackHost {
    host: Arc<FakeHost>,
    reads: std::sync::Mutex<std::collections::VecDeque<Option<Output>>>,
}

#[async_trait::async_trait]
impl Runner for ReadbackHost {
    async fn run(&self, program: &'static str, args: &[&'static str]) -> std::io::Result<Output> {
        let actual = self.host.run(program, args).await?;
        if program == PMSET && args == ["-g", "custom"] {
            if let Some(Some(out)) = self.reads.lock().unwrap().pop_front() {
                return Ok(out);
            }
        }
        Ok(actual)
    }
}

fn read_output(stdout: &str) -> Output {
    Output {
        success: true,
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

#[tokio::test]
async fn unreadable_before_state_refuses_every_write_and_disables_the_report() {
    for out in [
        read_output(""),
        read_output("AC Power:\n powermode 0\nBattery Power:\n standby 1\n"),
        read_output("AC Power:\n powermode 3\n"),
        Output {
            success: false,
            stderr: "read failed".into(),
            ..Default::default()
        },
    ] {
        let host = FakeHost::mac();
        host.install_rule();
        let runner = Arc::new(ReadbackHost {
            host: host.clone(),
            reads: std::sync::Mutex::new([Some(out.clone()), Some(out)].into()),
        });
        let power = HostPower::with_runner(runner, "factory");
        let report = power.read().await;
        assert!(!report.can_change);
        assert!(!report.notes.is_empty());
        assert!(power.set(PowerMode::HighPerformance).await.is_err());
        assert!(host.writes().is_empty());
    }
}

#[tokio::test]
async fn a_write_is_journaled_but_not_reported_successful_if_readback_fails_or_disagrees() {
    for out in [
        read_output(""),
        read_output("AC Power:\n powermode 2\n"), // battery disappeared
        read_output("AC Power:\n powermode 2\nBattery Power:\n powermode 1\n"),
        read_output("AC Power:\n powermode 2\nBattery Power:\n powermode 3\n"),
        Output {
            success: false,
            stderr: "read failed".into(),
            ..Default::default()
        },
    ] {
        let engine = engine();
        let host = FakeHost::mac();
        host.install_rule();
        engine.host_power.replace_runner(Arc::new(ReadbackHost {
            host: host.clone(),
            reads: std::sync::Mutex::new([None, Some(out)].into()),
        }));
        let response = engine
            .handle_request(Request::HostPowerModeSet {
                mode: PowerMode::HighPerformance,
            })
            .await;
        assert!(matches!(response, Response::Error { .. }), "{response:?}");
        assert_eq!(host.writes().len(), 1);
        let entries = engine.store.entries(HOST_JOURNAL, 20).await.unwrap();
        assert_eq!(entries.len(), 1);
        let data = entries[0].data.as_ref().unwrap();
        assert_eq!(data["confirmed"], false);
        assert_eq!(data["from_ac"], "automatic");
        assert_eq!(data["to"], "high_performance");
        assert!(
            data["verification_error"]
                .as_str()
                .unwrap()
                .contains("read-back")
        );
        assert!(entries[0].message.contains("read-back"));
    }
}

#[tokio::test]
async fn desktop_readback_confirms_its_only_source() {
    let host = FakeHost::mac();
    *host.battery.lock().unwrap() = None;
    host.install_rule();
    let power = HostPower::with_runner(host, "factory");
    let changed = power.set(PowerMode::EnergySaving).await.unwrap();
    assert!(changed.verification_error.is_none());
    assert_eq!(changed.after.ac, Some(PowerMode::EnergySaving));
    assert_eq!(changed.after.battery, None);
}

#[test]
fn account_names_in_copy_paste_commands_must_be_shell_and_sudoers_safe() {
    for safe in ["factory", "ingo", "user.name", "_system", "a-b"] {
        assert!(safe_account_name(safe));
    }
    for unsafe_name in [
        "",
        "-option",
        "a'b",
        "a b",
        "$(command)",
        "a\nALL",
        "a,b",
        "a\\b",
    ] {
        assert!(!safe_account_name(unsafe_name));
    }
    assert!(parse_admins("GroupMembership: $(command) -option\n").is_empty());
}

#[test]
fn admins_are_the_admin_groups_people_not_root_or_system_accounts() {
    assert_eq!(
        parse_admins("GroupMembership: root ingo _mbsetupuser\n"),
        vec!["ingo"]
    );
    // Long memberships continue on indented lines.
    assert_eq!(
        parse_admins("GroupMembership:\n root ingo\n ada _spotlight ingo\n"),
        vec!["ingo", "ada"]
    );
    assert!(parse_admins("GroupMembership: root _mbsetupuser\n").is_empty());
    assert!(parse_admins("No such key: GroupMembership\n").is_empty());
}

/// The three cases the install step is worded for, end to end through a
/// read: the daemon's user is not an admin (this host: `factory`, admin
/// `ingo`), it is one, or no admin can be found.
#[tokio::test]
async fn the_install_step_names_an_administrator_when_the_daemon_user_is_not_one() {
    let host = FakeHost::mac();
    let report = HostPower::with_runner(host.clone(), "factory").read().await;
    let s = &report.sudoers;
    assert_eq!(s.admins, vec!["ingo"]);
    assert!(!s.user_is_admin);
    assert_eq!(s.switch_to(), Some("ingo"));
    let steps = s.install_steps();
    assert_eq!(steps.len(), 3, "{steps:?}");
    // `su` alone, in a block of its own: pasted with the install, its
    // password prompt swallows the rest and nothing runs.
    assert_eq!(steps[0].text, "Run this alone and enter ingo's password:");
    assert_eq!(steps[0].command.as_deref(), Some("su - ingo"));
    assert_eq!(steps[1].text, "Then, in that shell, paste:");
    assert_eq!(steps[1].command.as_deref(), Some(s.install.as_str()));
    assert!(!steps[1].command.as_deref().unwrap().contains("su - "));
    assert_eq!(steps[2].text, "Then `exit`, and Refresh.");
    assert_eq!(steps[2].command, None);
    // The rule still names the daemon's own user, at the same path.
    assert!(
        s.rule.starts_with("factory ALL=(root) NOPASSWD: "),
        "{}",
        s.rule
    );
    assert_eq!(s.path, "/etc/sudoers.d/factory-pmset");
    assert!(host.calls().contains(&vec![
        DSCL,
        ".",
        "-read",
        "/Groups/admin",
        "GroupMembership"
    ]));

    let admin = HostPower::with_runner(FakeHost::mac(), "ingo").read().await;
    assert!(admin.sudoers.user_is_admin);
    assert_eq!(admin.sudoers.switch_to(), None);
    let steps = admin.sudoers.install_steps();
    assert_eq!(steps.len(), 1);
    assert!(
        steps[0].text.ends_with("root-owned and read-only:"),
        "{}",
        steps[0].text
    );
    assert_eq!(
        steps[0].command.as_deref(),
        Some(admin.sudoers.install.as_str())
    );

    let lonely = std::sync::Arc::new(FakeHost {
        cap: MAC_CAP.into(),
        ac: std::sync::Mutex::new(Some("0")),
        battery: std::sync::Mutex::new(Some("0")),
        permitted: std::sync::Mutex::new(Vec::new()),
        admin_group: "GroupMembership: root _mbsetupuser\n".into(),
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let none = HostPower::with_runner(lonely, "factory").read().await;
    assert!(none.sudoers.admins.is_empty());
    assert!(!none.sudoers.user_is_admin);
    assert_eq!(none.sudoers.switch_to(), None);
    let steps = none.sudoers.install_steps();
    assert_eq!(steps.len(), 1);
    assert!(
        steps[0]
            .text
            .ends_with("Run it from an administrator account:"),
        "{}",
        steps[0].text
    );
}
