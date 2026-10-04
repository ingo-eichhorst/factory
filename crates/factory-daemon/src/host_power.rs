//! The host's macOS power mode -- the L1 Mac tab, issue #260.
//!
//! System Settings calls it *Energy Mode*: Automatic, High Power, Low Power.
//! It changes how fast every agent's build and test runs, so the owner wants
//! it next to the rest of the host rather than behind `sudo pmset` typed by
//! hand. `pmset` numbers them `powermode 0` (Automatic), `1` (Low Power,
//! energy saving) and `2` (High Power, high performance).
//!
//! ## Reading needs nothing, writing needs root
//!
//! `pmset -g custom` (each power source's settings) and `pmset -g cap` (which
//! settings this host has at all) need no privilege. `pmset -a powermode N`
//! does, and the daemon is a launchd job with no TTY and no GUI session, so
//! there is nobody to type a password and no `osascript ... with
//! administrator privileges` either. It runs `sudo -n`, which fails at once
//! instead of prompting, and works only once the owner has installed a
//! sudoers drop-in scoped to exactly the three commands (`SudoersRule`).
//! Whether that rule is there is found out with `sudo -n -l <command>`,
//! which *lists* whether the command would be permitted and never runs it --
//! not by trying a write.
//!
//! ## No free string reaches a command
//!
//! The `Runner` seam takes `&'static str` arguments only. Every argument
//! this module ever passes is a literal in this file -- the mode's number
//! comes from a `match` over the closed `PowerMode` enum -- so nothing a
//! caller sent can be spliced into an argument list, let alone a shell
//! (none is involved: the program is executed directly, by absolute path).
//!
//! ## The platform seam
//!
//! The same shape as `crate::power`: a small trait with one real body on
//! macOS and nothing anywhere else. Off macOS there is no runner, and the
//! report says "not applicable". Tests never get the real one either: under
//! `cfg(test)` every `HostPower` starts with no runner, and a test that wants
//! one hands it a fake -- `cargo test` must never run a real `pmset` or
//! `sudo` on whoever's machine it is.

use std::sync::Arc;
#[cfg(target_os = "macos")]
use std::time::Duration;

use factory_core::error::{FactoryError, Result};
use factory_core::protocol::{PowerMode, PowerModeReport, SudoersRule};

/// `pmset`, by absolute path: the one the sudoers rule names.
pub(crate) const PMSET: &str = "/usr/bin/pmset";
/// `sudo`, by absolute path: a launchd job's `PATH` is not to be trusted.
pub(crate) const SUDO: &str = "/usr/bin/sudo";
/// Where the drop-in goes.
pub(crate) const SUDOERS_PATH: &str = "/etc/sudoers.d/factory-pmset";
/// Long enough for a busy host; short enough that a page never hangs on it.
#[cfg(target_os = "macos")]
const TIMEOUT: Duration = Duration::from_secs(10);

/// What a finished command said.
#[derive(Debug, Clone, Default)]
pub(crate) struct Output {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// The platform seam: run one program with fixed arguments. `Err` is "it
/// could not be run at all" (missing, timed out); a program that ran and
/// failed is `Ok` with `success: false`.
#[async_trait::async_trait]
pub(crate) trait Runner: Send + Sync {
    async fn run(&self, program: &'static str, args: &[&'static str]) -> std::io::Result<Output>;
}

/// macOS: the program itself, executed directly -- no shell -- with no stdin
/// (so nothing can wait on a prompt), killed if the daemon stops waiting.
#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
struct SystemRunner;

#[cfg(target_os = "macos")]
#[async_trait::async_trait]
impl Runner for SystemRunner {
    async fn run(&self, program: &'static str, args: &[&'static str]) -> std::io::Result<Output> {
        let child = tokio::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .output();
        match tokio::time::timeout(TIMEOUT, child).await {
            Ok(Ok(out)) => Ok(Output {
                success: out.status.success(),
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("{program} did not answer within {}s", TIMEOUT.as_secs()),
            )),
        }
    }
}

/// The runner this build gets: the real one on macOS outside tests, none
/// anywhere else.
fn platform() -> Option<Arc<dyn Runner>> {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        Some(Arc::new(SystemRunner))
    }
    #[cfg(not(all(target_os = "macos", not(test))))]
    {
        None
    }
}

/// The `pmset powermode` number for a mode -- a literal per arm, so the
/// value handed to `sudo` is never built from anything a caller sent.
pub(crate) fn pmset_value(mode: PowerMode) -> &'static str {
    match mode {
        PowerMode::Automatic => "0",
        PowerMode::EnergySaving => "1",
        PowerMode::HighPerformance => "2",
    }
}

/// `-n` then the full command: run it as root without ever prompting.
pub(crate) fn sudo_set_args(mode: PowerMode) -> [&'static str; 5] {
    ["-n", PMSET, "-a", "powermode", pmset_value(mode)]
}

/// `-n -l` then the full command: would it be permitted? Lists, never runs.
pub(crate) fn sudo_probe_args(mode: PowerMode) -> [&'static str; 6] {
    ["-n", "-l", PMSET, "-a", "powermode", pmset_value(mode)]
}

fn from_pmset(value: &str) -> Option<PowerMode> {
    match value {
        "0" => Some(PowerMode::Automatic),
        "1" => Some(PowerMode::EnergySaving),
        "2" => Some(PowerMode::HighPerformance),
        _ => None,
    }
}

/// One power source's mode, as `pmset -g custom` printed it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Custom {
    pub ac: Option<PowerMode>,
    pub battery: Option<PowerMode>,
    /// A value this daemon does not know, in words.
    pub notes: Vec<String>,
}

/// `pmset -g custom`: a `Battery Power:` and/or an `AC Power:` heading (a
/// Mac with no battery prints only the latter), each followed by indented
/// `name value` lines. `powermode` is the setting; a macOS too old to have
/// it reports `lowpowermode 0|1` instead, read as Automatic or Energy
/// saving. Any other section (`UPS Power:`) is ignored.
pub(crate) fn parse_custom(text: &str) -> Custom {
    #[derive(Clone, Copy, PartialEq)]
    enum Source {
        None,
        Ac,
        Battery,
        Other,
    }
    let mut out = Custom::default();
    let mut source = Source::None;
    // Per section: (powermode, lowpowermode) as written.
    let mut seen: [(Option<String>, Option<String>); 2] = Default::default();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !line.starts_with(char::is_whitespace) && trimmed.ends_with(':') {
            source = match trimmed {
                "AC Power:" => Source::Ac,
                "Battery Power:" => Source::Battery,
                _ => Source::Other,
            };
            continue;
        }
        let slot = match source {
            Source::Ac => 0,
            Source::Battery => 1,
            Source::None | Source::Other => continue,
        };
        let mut words = trimmed.split_whitespace();
        match (words.next(), words.next()) {
            (Some("powermode"), Some(v)) => seen[slot].0 = Some(v.to_string()),
            (Some("lowpowermode"), Some(v)) => seen[slot].1 = Some(v.to_string()),
            _ => {}
        }
    }
    for (slot, name) in [(0, "AC"), (1, "battery")] {
        let (power, low) = &seen[slot];
        let mode = match (power, low) {
            (Some(v), _) => from_pmset(v).or_else(|| {
                out.notes.push(format!(
                    "{name} reports powermode {v}, which this daemon does not know"
                ));
                None
            }),
            (None, Some(v)) if v == "1" => Some(PowerMode::EnergySaving),
            (None, Some(v)) if v == "0" => Some(PowerMode::Automatic),
            (None, Some(v)) => {
                out.notes.push(format!(
                    "{name} reports lowpowermode {v}, which this daemon does not know"
                ));
                None
            }
            (None, None) => None,
        };
        if slot == 0 {
            out.ac = mode;
        } else {
            out.battery = mode;
        }
    }
    out
}

/// `pmset -g cap`: one capability per indented line. Automatic is offered
/// whenever either other mode is; a host with neither has no energy mode.
pub(crate) fn parse_cap(text: &str) -> Vec<PowerMode> {
    let caps: Vec<&str> = text.lines().map(str::trim).collect();
    let high = caps.contains(&"highpowermode");
    let low = caps.contains(&"lowpowermode");
    PowerMode::ALL
        .into_iter()
        .filter(|m| match m {
            PowerMode::Automatic => high || low,
            PowerMode::HighPerformance => high,
            PowerMode::EnergySaving => low,
        })
        .collect()
}

/// The drop-in for `user`: exactly the three commands, as root, without a
/// password -- and the commands that check it before installing it.
pub(crate) fn sudoers_rule(user: &str) -> SudoersRule {
    let commands: Vec<String> = PowerMode::ALL
        .into_iter()
        .map(pmset_value)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(|n| format!("{PMSET} -a powermode {n}"))
        .collect();
    let rule = format!("{user} ALL=(root) NOPASSWD: {}", commands.join(", "));
    // Checked with `visudo -cf` on a private temporary copy first: a broken
    // file in /etc/sudoers.d can lock every `sudo` on the machine out, so it
    // only reaches its place once it parses.
    let install = format!(
        "f=\"$(mktemp)\" && printf '%s\\n' '{rule}' > \"$f\" && sudo visudo -cf \"$f\" && \
         sudo install -m 0440 -o root -g wheel \"$f\" {SUDOERS_PATH}; rm -f \"$f\""
    );
    SudoersRule {
        path: SUDOERS_PATH.to_string(),
        user: user.to_string(),
        rule,
        install,
        check: "sudo visudo -c".to_string(),
    }
}

/// The user this daemon runs as -- the one the rule has to name.
fn current_user() -> String {
    #[cfg(unix)]
    {
        // SAFETY: getpwuid_r writes into `pwd` and `buf`, both valid for the
        // sizes given, and `result` is only read when the call says it found
        // an entry, in which case `pw_name` points into `buf`.
        let uid = unsafe { libc::geteuid() };
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut buf = [0 as libc::c_char; 1024];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        let rc =
            unsafe { libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr(), buf.len(), &mut result) };
        if rc == 0 && !result.is_null() && !pwd.pw_name.is_null() {
            let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) }
                .to_string_lossy()
                .into_owned();
            if !name.is_empty() {
                return name;
            }
        }
    }
    std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| "factory".to_string())
}

/// What `Engine` holds. One runner, and one lock so two clicks cannot
/// interleave their read-set-read sequences.
pub struct HostPower {
    runner: std::sync::RwLock<Option<Arc<dyn Runner>>>,
    user: String,
    set_lock: tokio::sync::Mutex<()>,
}

impl Default for HostPower {
    fn default() -> Self {
        Self::new()
    }
}

/// The before and after of one change, for the journal.
pub(crate) struct Changed {
    pub before: Custom,
    pub after: Custom,
}

impl HostPower {
    pub fn new() -> Self {
        Self {
            runner: std::sync::RwLock::new(platform()),
            user: current_user(),
            set_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// A test's fake in place of whatever this build has.
    #[cfg(test)]
    pub(crate) fn with_runner(runner: Arc<dyn Runner>, user: &str) -> Self {
        Self {
            runner: std::sync::RwLock::new(Some(runner)),
            user: user.to_string(),
            set_lock: tokio::sync::Mutex::new(()),
        }
    }

    /// Swap the runner on an engine a test already built.
    #[cfg(test)]
    pub(crate) fn replace_runner(&self, runner: Arc<dyn Runner>) {
        *self.runner.write().unwrap() = Some(runner);
    }

    fn runner(&self) -> Option<Arc<dyn Runner>> {
        self.runner.read().map(|r| r.clone()).unwrap_or(None)
    }

    async fn cap(runner: &dyn Runner, notes: &mut Vec<String>) -> Vec<PowerMode> {
        match runner.run(PMSET, &["-g", "cap"]).await {
            Ok(out) if out.success => parse_cap(&out.stdout),
            Ok(out) => {
                notes.push(format!("pmset -g cap failed: {}", out.stderr.trim()));
                Vec::new()
            }
            Err(e) => {
                notes.push(format!("pmset -g cap could not run: {e}"));
                Vec::new()
            }
        }
    }

    async fn custom(runner: &dyn Runner) -> std::result::Result<Custom, String> {
        match runner.run(PMSET, &["-g", "custom"]).await {
            Ok(out) if out.success => Ok(parse_custom(&out.stdout)),
            Ok(out) => Err(format!("pmset -g custom failed: {}", out.stderr.trim())),
            Err(e) => Err(format!("pmset -g custom could not run: {e}")),
        }
    }

    /// Would `sudo -n` run the command that sets `mode`? Anything but a
    /// clean yes -- no rule, a password wanted, no `sudo`, a timeout -- is
    /// no. Only "could not ask at all" is worth a note: "not permitted" is
    /// the ordinary state before the rule is installed.
    async fn permitted(runner: &dyn Runner, mode: PowerMode, notes: &mut Vec<String>) -> bool {
        match runner.run(SUDO, &sudo_probe_args(mode)).await {
            Ok(out) => out.success,
            Err(e) => {
                let note = format!("sudo could not be asked: {e}");
                if !notes.contains(&note) {
                    notes.push(note);
                }
                false
            }
        }
    }

    /// The report, read fresh. `changes` is left empty for the engine to
    /// fill from its journal.
    pub(crate) async fn read(&self) -> PowerModeReport {
        let sudoers = sudoers_rule(&self.user);
        let Some(runner) = self.runner() else {
            return PowerModeReport {
                applicable: false,
                supported: Vec::new(),
                ac: None,
                battery: None,
                permitted: Vec::new(),
                can_change: false,
                sudoers,
                notes: vec![
                    "The power mode is a macOS setting; this host is not macOS.".to_string()
                ],
                changes: Vec::new(),
            };
        };
        let mut notes = Vec::new();
        let supported = Self::cap(runner.as_ref(), &mut notes).await;
        let custom = match Self::custom(runner.as_ref()).await {
            Ok(c) => c,
            Err(e) => {
                notes.push(e);
                Custom::default()
            }
        };
        notes.extend(custom.notes.iter().cloned());
        let mut permitted = Vec::new();
        for mode in &supported {
            if Self::permitted(runner.as_ref(), *mode, &mut notes).await {
                permitted.push(*mode);
            }
        }
        let can_change = !supported.is_empty() && supported.iter().all(|m| permitted.contains(m));
        PowerModeReport {
            applicable: true,
            supported,
            ac: custom.ac,
            battery: custom.battery,
            permitted,
            can_change,
            sudoers,
            notes,
            changes: Vec::new(),
        }
    }

    /// Set `mode` on every power source: refuse a mode the host does not
    /// offer or the rule does not permit before anything runs, then
    /// `sudo -n pmset -a powermode N`, then read it back.
    pub(crate) async fn set(&self, mode: PowerMode) -> Result<Changed> {
        let _held = self.set_lock.lock().await;
        let Some(runner) = self.runner() else {
            return Err(FactoryError::BadRequest(
                "the power mode is a macOS setting; this host is not macOS".into(),
            ));
        };
        let mut notes = Vec::new();
        let supported = Self::cap(runner.as_ref(), &mut notes).await;
        if !supported.contains(&mode) {
            return Err(FactoryError::BadRequest(match notes.first() {
                Some(why) => format!("cannot tell whether this host offers {mode}: {why}"),
                None => format!("this host does not offer {mode} (pmset -g cap)"),
            }));
        }
        if !Self::permitted(runner.as_ref(), mode, &mut notes).await {
            return Err(FactoryError::BadRequest(format!(
                "Factory may not change the power mode yet: install the sudoers rule at {SUDOERS_PATH} \
                 (shown on the L1 Mac tab) so `sudo -n {PMSET} -a powermode {}` is permitted",
                pmset_value(mode)
            )));
        }
        let before = Self::custom(runner.as_ref()).await.unwrap_or_default();
        let out = runner
            .run(SUDO, &sudo_set_args(mode))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("sudo could not run pmset: {e}")))?;
        if !out.success {
            let why = out.stderr.trim();
            return Err(FactoryError::Other(anyhow::anyhow!(
                "pmset -a powermode {} failed{}",
                pmset_value(mode),
                if why.is_empty() {
                    String::new()
                } else {
                    format!(": {why}")
                }
            )));
        }
        let after = Self::custom(runner.as_ref()).await.unwrap_or_default();
        Ok(Changed { before, after })
    }
}

/// The journal a power-mode change is recorded in -- not a task, so a
/// fixed id of its own, like the secrets catalogue's.
pub(crate) const HOST_JOURNAL: &str = "factory:host";
/// The entry kind of one change.
pub(crate) const POWER_MODE_CHANGED: &str = "power_mode_changed";
/// How many changes the tab shows.
const CHANGES_SHOWN: u32 = 20;

fn shown(mode: Option<PowerMode>) -> &'static str {
    mode.map(PowerMode::label).unwrap_or("unknown")
}

/// `Automatic`, or `AC Automatic, battery Energy saving` when they differ.
fn modes_words(c: &Custom) -> String {
    match (c.ac, c.battery) {
        (a, b) if a == b => shown(a).to_string(),
        (a, None) => shown(a).to_string(),
        (None, b) => format!("battery {}", shown(b)),
        (a, b) => format!("AC {}, battery {}", shown(a), shown(b)),
    }
}

fn mode_of(data: &serde_json::Value, key: &str) -> Option<PowerMode> {
    data.get(key)
        .cloned()
        .and_then(|v| serde_json::from_value(v).ok())
}

impl crate::engine::Engine {
    /// `Request::HostPowerMode`: read fresh, with the newest changes.
    pub(crate) async fn host_power_report(&self) -> PowerModeReport {
        let mut report = self.host_power.read().await;
        report.changes = self.power_mode_changes().await;
        report
    }

    async fn power_mode_changes(&self) -> Vec<factory_core::protocol::PowerModeChange> {
        let entries = self
            .store
            .entries(HOST_JOURNAL, CHANGES_SHOWN)
            .await
            .unwrap_or_default();
        entries
            .into_iter()
            .filter(|e| e.kind == POWER_MODE_CHANGED)
            .filter_map(|e| {
                let data = e.data.clone().unwrap_or_default();
                Some(factory_core::protocol::PowerModeChange {
                    at: e.at,
                    by: data
                        .get("by")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    from_ac: mode_of(&data, "from_ac"),
                    from_battery: mode_of(&data, "from_battery"),
                    to: mode_of(&data, "to")?,
                    message: e.message,
                })
            })
            .collect()
    }

    /// `Request::HostPowerModeSet`: set it, journal who changed it from
    /// what to what, and answer the report as it now reads.
    pub(crate) async fn set_host_power_mode(
        &self,
        caller: &crate::access::Caller,
        mode: PowerMode,
    ) -> Result<PowerModeReport> {
        let Changed { before, after } = self.host_power.set(mode).await?;
        let asked = crate::operations::Asked::new(caller, None);
        let message = format!(
            "power mode: {} -> {mode} {}",
            modes_words(&before),
            asked.words()
        );
        let entry = asked.entry(
            POWER_MODE_CHANGED,
            message,
            serde_json::json!({
                "from_ac": before.ac,
                "from_battery": before.battery,
                "to": mode,
                "after_ac": after.ac,
                "after_battery": after.battery,
            }),
        );
        if let Err(e) = self.store.append_entry(HOST_JOURNAL, &entry).await {
            tracing::warn!("the power mode was changed but not journaled: {e}");
        }
        let read_back = [after.ac, after.battery]
            .into_iter()
            .flatten()
            .all(|m| m == mode);
        if read_back {
            tracing::info!("power mode set to {mode} {}", asked.words());
        } else {
            tracing::warn!(
                "power mode set to {mode}, but pmset now reads {}",
                modes_words(&after)
            );
        }
        Ok(self.host_power_report().await)
    }
}

/// A pretend host for tests: answers `pmset -g cap`/`-g custom` and the
/// two `sudo` forms from its own state, records every command, and panics
/// on anything else -- so a test that reaches for an unexpected program
/// fails loudly instead of running it.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;

    pub(crate) const MAC_CAP: &str =
        "Capabilities for AC Power:\n displaysleep\n sleep\n lowpowermode\n highpowermode\n";

    pub(crate) struct FakeHost {
        pub cap: String,
        /// `powermode` per section as `pmset` would print it; `None` drops
        /// the section (a desktop Mac has no battery).
        pub ac: Mutex<Option<&'static str>>,
        pub battery: Mutex<Option<&'static str>>,
        /// The `powermode` values the sudoers rule permits.
        pub permitted: Mutex<Vec<&'static str>>,
        pub calls: Mutex<Vec<Vec<&'static str>>>,
    }

    impl FakeHost {
        /// This host today: both modes offered, Automatic on both sources,
        /// no sudoers rule.
        pub(crate) fn mac() -> Arc<Self> {
            Arc::new(Self {
                cap: MAC_CAP.to_string(),
                ac: Mutex::new(Some("0")),
                battery: Mutex::new(Some("0")),
                permitted: Mutex::new(Vec::new()),
                calls: Mutex::new(Vec::new()),
            })
        }

        pub(crate) fn install_rule(&self) {
            *self.permitted.lock().unwrap() = vec!["0", "1", "2"];
        }

        pub(crate) fn calls(&self) -> Vec<Vec<&'static str>> {
            self.calls.lock().unwrap().clone()
        }

        /// Every write -- a `sudo` that is not a `-l` listing.
        pub(crate) fn writes(&self) -> Vec<Vec<&'static str>> {
            self.calls()
                .into_iter()
                .filter(|c| c[0] == SUDO && c.get(2) != Some(&"-l"))
                .collect()
        }

        fn custom(&self) -> String {
            let mut out = String::new();
            if let Some(v) = *self.battery.lock().unwrap() {
                out.push_str(&format!("Battery Power:\n Sleep On Power Button 1\n powermode            {v}\n standby              1\n"));
            }
            if let Some(v) = *self.ac.lock().unwrap() {
                out.push_str(&format!("AC Power:\n Sleep On Power Button 1\n powermode            {v}\n standby              0\n"));
            }
            out
        }
    }

    fn ok(stdout: impl Into<String>) -> std::io::Result<Output> {
        Ok(Output {
            success: true,
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }

    fn refused() -> std::io::Result<Output> {
        Ok(Output {
            success: false,
            stdout: String::new(),
            stderr: "sudo: a password is required\n".into(),
        })
    }

    #[async_trait::async_trait]
    impl Runner for FakeHost {
        async fn run(
            &self,
            program: &'static str,
            args: &[&'static str],
        ) -> std::io::Result<Output> {
            let mut call = vec![program];
            call.extend_from_slice(args);
            self.calls.lock().unwrap().push(call);
            match (program, args) {
                (PMSET, ["-g", "cap"]) => ok(self.cap.clone()),
                (PMSET, ["-g", "custom"]) => ok(self.custom()),
                (SUDO, ["-n", "-l", PMSET, "-a", "powermode", n]) => {
                    if self.permitted.lock().unwrap().contains(n) {
                        ok(format!("{PMSET} -a powermode {n}\n"))
                    } else {
                        refused()
                    }
                }
                (SUDO, ["-n", PMSET, "-a", "powermode", n]) => {
                    if !self.permitted.lock().unwrap().contains(n) {
                        return refused();
                    }
                    let n: &'static str = n;
                    for source in [&self.ac, &self.battery] {
                        let mut slot = source.lock().unwrap();
                        if slot.is_some() {
                            *slot = Some(n);
                        }
                    }
                    ok("")
                }
                _ => panic!("the fake host was asked to run {program} {args:?}"),
            }
        }
    }
}

#[cfg(test)]
mod tests;
