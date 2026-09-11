//! ADR 0022 decision 10 / design §4: **no Factory command writes, creates, or
//! deletes a file outside a `.factory/` directory.**
//!
//! # Why this file lives in `factory-cli`, not `factory-e2e`
//!
//! The brief for this test named `factory-e2e` as the natural home. It is not
//! used here because the discovery mechanism this test exists to prove —
//! walking `factory_cli::cli::Cli::command()` via `clap::CommandFactory`, and
//! spawning the real `factory` binary — needs `factory-e2e` to depend on
//! `factory-cli`, which it does not, and adding that dependency means editing
//! `Cargo.toml`, which this task forbids. `factory-cli`'s own `tests/`
//! directory already has both for free: `env!("CARGO_BIN_EXE_factory")` (the
//! real binary) and, after one change, `factory_cli::cli::Cli` (the parser).
//! That one change is `lib.rs`'s `mod cli;` becoming `pub mod cli;` — a
//! visibility change with no behavioural effect, made solely so this file can
//! reach the type this ADR names by name. See this task's own report for the
//! deviation recorded against the brief.
//!
//! # What this test proves, and what it does not
//!
//! It runs the real `factory` binary against a real `factory daemon run`
//! process and a real, throwaway instance root (a fresh `tempfile::TempDir`,
//! never `/Users/factory/business-factory/.factory/`). Before and after every
//! command it snapshots that instance root recursively — every path, its
//! length, and a content hash — and asserts that anything created, deleted,
//! or changed sits under `.factory/`.
//!
//! **What it does not show.** It watches only the scratch instance root. A
//! command that wrote into `$HOME`, `/tmp`, or anywhere else on the
//! filesystem would be invisible to it. It also does not run `agent start`
//! for real (see the exemption table below), so no test here ever proves that
//! a *delivered* prompt stays inside a session's own workspace — design §4's
//! own clarification is that an agent's work product is exempt from this rule
//! entirely, and this test does not touch that path at all. A reader must not
//! conclude from a green run here that Factory writes nowhere else on the
//! machine — only that, for every command this table could invoke, nothing
//! moved outside `.factory/` within the instance root this run controlled.
//!
//! # Registered scope directories, and why this test still lists them
//! explicitly even though a single recursive walk already covers them
//!
//! `factory_registry::resolve` joins every scope's declared `path` onto the
//! instance root and refuses the result with `RegistryError::EscapesInstance`
//! unless it is the root itself or a descendant of it. So, in this build, a
//! registered scope's canonical path can never fall outside the instance
//! root, and the single recursive walk this test performs over the whole
//! instance root already visits every scope directory's contents — nothing
//! is missed by construction. This test still reads back each registered
//! scope's canonical path from the daemon (`scope.list`) and asserts it is
//! contained in the walked tree, stated as a checked fact rather than an
//! assumption: relying silently on another crate's constraint as this test's
//! only safety net would stop protecting anything the day that constraint is
//! loosened, and ADR 0022 decision 10 asks for scope directories to be
//! included in the snapshot regardless of whether the walk already reaches
//! them by another route.
//!
//! Mutation 3 (see this task's report) demonstrates the failure mode this
//! guards against directly: a snapshot restricted to `.factory/` alone —
//! the mistake an implementer reaches for when "the rule is about
//! `.factory/`" — never visits a registered scope's directory at all, and a
//! write there goes completely unseen. This test does not make that mistake:
//! it walks the whole instance root, not `.factory/` in isolation.
//!
//! # The two clarifications design §4 states, honoured here
//!
//! - `factory init` may create the instance directory that will contain
//!   `.factory/`; creating the container is not writing content beside it.
//!   This test sidesteps the question entirely rather than resolving it: the
//!   instance root and `.factory/config.yaml` already exist (written by this
//!   test's own fixture setup, not by any `factory` command) before `init` is
//!   ever invoked, so the `init` call exercised here is the ordinary
//!   already-initialized case — itself a real, meaningful invocation of the
//!   leaf, just not the one clarification 1 is about.
//! - The rule constrains Factory, not agents: an agent's own work product is
//!   out of scope. No command exercised below ever delivers a prompt to a
//!   live session, so this distinction never actually comes up in this run —
//!   noted here so a reader does not read silence on it as an assertion that
//!   agent work product also stays inside `.factory/`. It plainly does not,
//!   and is not supposed to.
//!
//! # The exemption table
//!
//! `daemon run` blocks forever holding the socket until signalled — this
//! test drives it only through `factory start`/`factory stop`, and the
//! socket/lock/log files `start` causes `daemon run` to create are captured
//! and checked as part of the `start` step. `agent attach` execs a terminal
//! in place of this process and never returns on success. `agent start`
//! spawns a real Herdr pane running a real harness process; this machine
//! happens to have `herdr` and `pi` installed, but a live session still needs
//! a pre-configured, logged-in harness this test cannot set up hermetically,
//! which is exactly why the crate's own `live_herdr.rs`/`live_agent.rs`
//! drills gate the identical operation behind `#[ignore]` and a
//! human-prepared pane, and are never run by `check.sh`. Every command below
//! that needs an existing session or task state gets one seeded directly into
//! `sessions`/`tasks` by SQL instead — "it needs a running session" is not a
//! reason to skip a command, and none of the three exemptions above rest on
//! that reason.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use clap::CommandFactory;
use factory_cli::cli::Cli;
use serde_json::Value;
use uuid::Uuid;

// --- discovering the command surface (ADR 0022 decision 10) ----------------

/// Every leaf subcommand `clap` knows about, as the sequence of names from
/// the root down to it (`["task", "send"]`) — walked from the parser itself,
/// never hand-listed, so a command added later has nowhere to hide.
fn discover_leaves() -> Vec<Vec<String>> {
    fn walk(cmd: &clap::Command, prefix: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        let subs: Vec<&clap::Command> = cmd
            .get_subcommands()
            .filter(|s| !s.is_hide_set() && s.get_name() != "help")
            .collect();
        if subs.is_empty() {
            out.push(prefix.clone());
            return;
        }
        for sub in subs {
            prefix.push(sub.get_name().to_string());
            walk(sub, prefix, out);
            prefix.pop();
        }
    }

    let root = Cli::command();
    let mut out = Vec::new();
    walk(&root, &mut Vec::new(), &mut out);
    out
}

fn leaf(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// A leaf that cannot be invoked in this test's shape, and why. See this
/// file's module docs for the reasoning behind each entry — kept short here
/// on purpose, for the reader who is here only to add the next command.
const EXEMPT: &[(&[&str], &str)] = &[
    (
        &["daemon", "run"],
        "blocks forever holding the socket until signalled; exercised only through `start`/`stop`",
    ),
    (
        &["agent", "attach"],
        "execs a terminal in place of this process and never returns on success",
    ),
    (
        &["agent", "start"],
        "spawns a real Herdr pane running a real harness process; needs a pre-configured, \
         logged-in harness this test cannot set up hermetically (see `live_herdr.rs`/\
         `live_agent.rs`, both `#[ignore]`d for the same reason)",
    ),
];

// --- snapshotting a directory tree, by content -------------------------------

#[derive(Clone, PartialEq, Eq, Debug)]
enum Kind {
    File,
    Dir,
    /// A socket, FIFO, symlink, or device node — its presence and type are
    /// recorded, but it is never opened: reading `.factory/factory.sock`
    /// would hang or error, not reveal a write.
    Other,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct Entry {
    kind: Kind,
    len: u64,
    /// Content hash for a regular file, `0` otherwise. `DefaultHasher`
    /// (SipHash) is std-only — this is a change detector for a test, not a
    /// security boundary, so no new dependency is warranted.
    hash: u64,
}

type Snapshot = BTreeMap<PathBuf, Entry>;

fn hash_bytes(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// Snapshot `root` recursively: every path relative to `root`, its kind,
/// length, and — for a regular file — a hash of its actual bytes. Content,
/// not metadata: a rewrite that lands on the same mtime with different bytes
/// must still show up as a change (mutation 4 in this task's report proves
/// this the other way, by comparing mtime instead).
fn snapshot(root: &Path) -> Snapshot {
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

fn visit(root: &Path, dir: &Path, out: &mut Snapshot) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            out.insert(
                rel.to_path_buf(),
                Entry {
                    kind: Kind::Dir,
                    len: 0,
                    hash: 0,
                },
            );
            visit(root, &path, out);
        } else if meta.is_file() {
            let bytes = std::fs::read(&path).unwrap_or_default();
            out.insert(
                rel.to_path_buf(),
                Entry {
                    kind: Kind::File,
                    len: bytes.len() as u64,
                    hash: hash_bytes(&bytes),
                },
            );
        } else {
            out.insert(
                rel.to_path_buf(),
                Entry {
                    kind: Kind::Other,
                    len: meta.len(),
                    hash: 0,
                },
            );
        }
    }
}

fn changed_paths(before: &Snapshot, after: &Snapshot) -> Vec<PathBuf> {
    let mut keys: BTreeSet<PathBuf> = before.keys().cloned().collect();
    keys.extend(after.keys().cloned());
    keys.into_iter()
        .filter(|p| before.get(p) != after.get(p))
        .collect()
}

/// The whole test's assertion, run after every step: whatever changed must
/// sit under `.factory/`. `label` names the command (or fixture step) that
/// produced this snapshot, so a failure points at the one call responsible,
/// not merely at "something, somewhere".
fn assert_only_dot_factory_changed(before: &Snapshot, after: &Snapshot, label: &str) {
    let factory = std::ffi::OsStr::new(".factory");
    let offenders: Vec<PathBuf> = changed_paths(before, after)
        .into_iter()
        .filter(|p| p.components().next().map(|c| c.as_os_str()) != Some(factory))
        .collect();
    assert!(
        offenders.is_empty(),
        "`{label}` created, deleted, or changed a path outside `.factory/`: {offenders:?}"
    );
}

// --- running the real binary -------------------------------------------------

struct Invocation {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

impl Invocation {
    fn assert_ok(&self, label: &str) {
        assert!(
            self.status.success(),
            "`{label}` exited {:?}\nstdout: {}\nstderr: {}",
            self.status.code(),
            self.stdout,
            self.stderr
        );
    }

    /// `knowledge write`/`memory add`'s `path`/`name` field, and every other
    /// leaf's own id fields, are read out of the pretty-printed JSON
    /// `rpc::print_result` writes after its one-line headline — this parses
    /// that block back out rather than re-deriving ids the CLI already
    /// minted and told us about.
    fn json(&self) -> Value {
        let start = self.stdout.find('{').unwrap_or_else(|| {
            panic!("no JSON result in stdout: {}", self.stdout);
        });
        serde_json::from_str(&self.stdout[start..])
            .unwrap_or_else(|e| panic!("stdout JSON did not parse: {e}\nstdout: {}", self.stdout))
    }
}

fn factory_cmd(root: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_factory"));
    cmd.arg("--root").arg(root);
    cmd
}

fn run(root: &Path, args: &[&str]) -> Invocation {
    let output = factory_cmd(root)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("spawn `factory {}`: {e}", args.join(" ")));
    Invocation {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn run_with_stdin(root: &Path, args: &[&str], body: &str) -> Invocation {
    let mut child = factory_cmd(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn `factory {}`: {e}", args.join(" ")));
    child
        .stdin
        .take()
        .expect("child stdin is piped")
        .write_all(body.as_bytes())
        .expect("write stdin body");
    let output = child.wait_with_output().expect("wait for child");
    Invocation {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

// --- the scratch instance's own scaffolding, seeded directly by SQL --------
//
// A fixed, small seed rather than `uuid::Uuid::new_v4()` — this workspace
// pins `uuid` without the `v4`/`v7` feature everywhere, and every other test
// in this crate mints ids the same way.
fn uid(seed: u32) -> Uuid {
    Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

/// Insert one `sessions` row (and the `workspace_leases` row a live session
/// always carries) directly by SQL — the same shortcut
/// `factory-e2e/tests/live_herdr.rs::seed_disconnected_session` and this
/// crate's own `tests/cli.rs::seed_a_schedule_with_an_unreadable_timezone`
/// take, so a command that needs an existing session can be exercised
/// without ever starting a real Herdr pane (which `agent start` is exempted
/// from doing in this test — see the module docs).
///
/// `herdr_pane_id` is left `NULL` on purpose, never a made-up pane string.
/// Read directly off `handler::pane::read`, `ops::agent::stop`,
/// `handler/deliver.rs::AdapterPromptWriter`, and `observe::observe`: every
/// one of them calls the real adapter only when a session has a recorded
/// pane, and a `NULL` column is exactly the "nothing recorded yet" case each
/// of them already treats as "nothing to do" rather than an error. A fake
/// pane string would instead make every one of those call sites shell out to
/// the real `herdr` on whatever machine runs this test, which is precisely
/// the dependency `agent start`'s own exemption exists to avoid.
fn seed_session(db_path: &Path, id: Uuid, scope_id: Uuid, agent_name: &str, workspace: &Path) {
    let mut store = factory_store::Store::open_at(db_path).expect("open store read-write");
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state) \
         VALUES (?1, ?2, ?3, ?4, 'running')",
        (
            id.to_string(),
            scope_id.to_string(),
            agent_name,
            workspace.display().to_string(),
        ),
    )
    .expect("seed session");
    tx.execute(
        "INSERT INTO workspace_leases (session_id, canonical_workspace_path) VALUES (?1, ?2)",
        (id.to_string(), workspace.display().to_string()),
    )
    .expect("seed lease");
    tx.commit().expect("commit");
}

/// Insert one `tasks` row directly by SQL, in whatever status a downstream
/// `task` leaf needs as its precondition (`running` for `done`/`fail`/
/// `block`, `blocked`/`interrupted` for `resume`, `queued` for `assign`,
/// `done` for `rework`'s "the referenced run must be terminal"). See this
/// file's module docs: seeding state directly, rather than reaching it
/// through a live delivery, is exactly what lets these commands run without
/// a live Herdr session.
#[allow(clippy::too_many_arguments)]
fn seed_task(
    db_path: &Path,
    id: Uuid,
    target_scope_id: Uuid,
    status: &str,
    blocked_reason: Option<&str>,
    assigned_session_id: Option<Uuid>,
    prompt: &str,
) {
    let mut store = factory_store::Store::open_at(db_path).expect("open store read-write");
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO tasks (id, target_scope_id, status, blocked_reason, assigned_session_id, prompt) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (
            id.to_string(),
            target_scope_id.to_string(),
            status,
            blocked_reason,
            assigned_session_id.map(|s| s.to_string()),
            prompt,
        ),
    )
    .expect("seed task");
    tx.commit().expect("commit");
}

/// A running daemon this test started, killed as a last resort if the test
/// never got to call `factory stop` itself (a panic partway through, most
/// likely) — mirrors the `Drop` safety nets `tests/cli.rs::RealDaemon` and
/// `factory-e2e/tests/daemon_process_drill.rs::DaemonProcess` already use,
/// adapted to a daemon this test launched via `factory start` rather than by
/// holding its `Child` directly (`start` intentionally detaches).
struct DaemonGuard {
    root: PathBuf,
    stopped: bool,
}

impl DaemonGuard {
    fn stopped_via_cli(root: PathBuf) -> Self {
        Self {
            root,
            stopped: false,
        }
    }
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        if let Ok(pid_text) = std::fs::read_to_string(self.root.join(".factory/factory.lock")) {
            if let Ok(pid) = pid_text.trim().parse::<i64>() {
                let _ = Command::new("/bin/kill")
                    .arg("-9")
                    .arg(pid.to_string())
                    .status();
            }
        }
    }
}

/// Drives the whole script: run a command (or seed a fixture row), snapshot,
/// and assert nothing moved outside `.factory/` — attributed to whichever
/// step just ran, per this task's own instruction to snapshot between
/// commands rather than only at the start and end.
struct Rig {
    root: PathBuf,
    db_path: PathBuf,
    invoked: HashSet<Vec<String>>,
    last: Snapshot,
}

impl Rig {
    fn new(root: PathBuf) -> Self {
        let db_path = root.join(".factory/factory.sqlite");
        let last = snapshot(&root);
        Self {
            root,
            db_path,
            invoked: HashSet::new(),
            last,
        }
    }

    fn checkpoint(&mut self, label: &str) {
        let after = snapshot(&self.root);
        assert_only_dot_factory_changed(&self.last, &after, label);
        self.last = after;
    }

    /// Run `args`, mark `leaf_parts` invoked, checkpoint, and return the
    /// result for the caller to assert on (exit code, parse an id out of its
    /// JSON, and so on).
    fn step(&mut self, leaf_parts: &[&str], args: &[&str]) -> Invocation {
        let inv = run(&self.root, args);
        self.invoked.insert(leaf(leaf_parts));
        self.checkpoint(&format!("factory {}", args.join(" ")));
        inv
    }

    fn step_with_stdin(&mut self, leaf_parts: &[&str], args: &[&str], body: &str) -> Invocation {
        let inv = run_with_stdin(&self.root, args, body);
        self.invoked.insert(leaf(leaf_parts));
        self.checkpoint(&format!("factory {} <stdin>", args.join(" ")));
        inv
    }

    /// Fixture setup that is not a `factory` command at all (raw SQL against
    /// `factory.sqlite`, which always sits under `.factory/`) — still
    /// checkpointed, so a mistake in the seed itself is never silently
    /// blamed on the command that runs next.
    fn fixture(&mut self, label: &str, f: impl FnOnce(&Path)) {
        f(&self.db_path);
        self.checkpoint(&format!("fixture: {label}"));
    }
}

// --- the test itself ---------------------------------------------------------

#[test]
fn no_factory_command_writes_outside_dot_factory() {
    let leaves = discover_leaves();

    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().to_path_buf();

    let root_id = uid(1);
    let alpha_id = uid(2);

    // Fixture setup: raw filesystem writes, not `factory` commands. This is
    // the same shortcut `factory-e2e/tests/common.rs::build` takes, adapted
    // to hand-write `config.yaml` before `init` ever runs (see module docs,
    // clarification 1).
    std::fs::create_dir_all(root.join(".factory")).expect("create .factory");
    std::fs::create_dir_all(root.join("alpha")).expect("create alpha");
    let ws1 = root.join("workspaces/s1");
    let ws2 = root.join("workspaces/s2");
    std::fs::create_dir_all(&ws1).expect("create workspace 1");
    std::fs::create_dir_all(&ws2).expect("create workspace 2");
    std::fs::write(
        root.join("AGENTS.md"),
        "# root\n\nRoot scope instructions.\n",
    )
    .expect("write root AGENTS.md");
    std::fs::write(
        root.join("alpha/AGENTS.md"),
        "# alpha\n\nAlpha scope instructions.\n",
    )
    .expect("write alpha AGENTS.md");

    let config_yaml = format!(
        r#"version: 1

instance:
  id: 99999999-9999-4999-8999-000000000099
  name: write-invariant-instance

scopes:
  - id: {root_id}
    name: root
    path: .
    agent:
      name: root-agent
      harness: pi
      max_sessions: 1

  - id: {alpha_id}
    name: alpha
    path: alpha
    agent:
      name: alpha-agent
      harness: pi
      max_sessions: 2
"#
    );
    std::fs::write(root.join(".factory/config.yaml"), config_yaml).expect("write config.yaml");

    let mut rig = Rig::new(root.clone());

    // --- init: root and config.yaml already exist (clarification 1 does not
    // apply to this call — see module docs).
    rig.step(&["init"], &["init"]).assert_ok("init");

    // --- start: launches the real daemon in the background. Every command
    // below, and the exempted `daemon run`'s own filesystem effect, runs
    // through this one daemon.
    rig.step(&["start"], &["start", "--harness", "pi"])
        .assert_ok("start");
    let mut daemon = DaemonGuard::stopped_via_cli(root.clone());

    // --- scope reconcile --apply: the load-bearing step every scope-name
    // resolution below depends on.
    rig.step(&["scope", "reconcile"], &["scope", "reconcile", "--apply"])
        .assert_ok("scope reconcile --apply");

    // --- scope list: read-only, and the check that a registered scope's
    // canonical path is contained in the tree this test already walks (see
    // module docs on why that is checked rather than assumed).
    let list = rig.step(&["scope", "list"], &["scope", "list"]);
    list.assert_ok("scope list");
    let scopes = list.json()["scopes"]
        .as_array()
        .expect("scopes array")
        .clone();
    assert_eq!(
        scopes.len(),
        2,
        "expected exactly root and alpha registered: {scopes:?}"
    );
    // Canonicalized, not the raw `TempDir` path: on macOS `/var` is itself a
    // symlink into `/private/var`, and every scope's own `canonical_path`
    // comes back already resolved through it — comparing against the raw
    // path would report a false escape on every run.
    let canonical_root = std::fs::canonicalize(&root).expect("canonicalize instance root");
    for scope in &scopes {
        let canonical = scope["canonical_path"]
            .as_str()
            .unwrap_or_else(|| panic!("scope has no canonical_path: {scope:?}"));
        assert!(
            PathBuf::from(canonical).starts_with(&canonical_root),
            "registered scope `{}` canonicalized to {canonical}, outside the instance root \
             {canonical_root:?} — factory_registry::resolve should have refused this with \
             EscapesInstance",
            scope["name"]
        );
    }

    // --- scope add: always refused (commands/scope.rs) — invoked anyway so
    // the refusal itself is proven to write nothing.
    let add = rig.step(
        &["scope", "add"],
        &["scope", "add", "--path", "somewhere", "--name", "zzz"],
    );
    assert!(
        !add.status.success(),
        "`scope add` must still be refused: {}",
        add.stderr
    );

    // --- status / doctor: read-only.
    rig.step(&["status"], &["status"]).assert_ok("status");
    // `doctor`'s exit code depends on host state (herdr, launchctl); only the
    // write invariant is this test's concern here.
    rig.step(&["doctor"], &["doctor"]);

    // --- agent list / status: read-only, no live session needed.
    rig.step(&["agent", "list"], &["agent", "list", "--scope", "alpha"])
        .assert_ok("agent list");
    rig.step(
        &["agent", "status"],
        &["agent", "status", "--scope", "alpha"],
    )
    .assert_ok("agent status");

    // --- knowledge write/list/show. `--source` names a file that genuinely
    // exists (`alpha/AGENTS.md`), so if the daemon ever starts checking a
    // source path's existence, this remains a read, never a write.
    rig.step_with_stdin(
        &["knowledge", "write"],
        &[
            "knowledge",
            "write",
            "--name",
            "write-invariant-note",
            "--title",
            "Write Invariant Test Note",
            "--status",
            "draft",
            "--source",
            "alpha/AGENTS.md",
        ],
        "this note exists only to prove `knowledge write` stays inside `.factory/knowledge/`.",
    )
    .assert_ok("knowledge write");
    rig.step(&["knowledge", "list"], &["knowledge", "list"])
        .assert_ok("knowledge list");
    rig.step(
        &["knowledge", "show"],
        &["knowledge", "show", "--name", "write-invariant-note"],
    )
    .assert_ok("knowledge show");

    // --- memory add/list.
    rig.step_with_stdin(
        &["memory", "add"],
        &["memory", "add", "--scope", "alpha"],
        "a scratch memory entry for the write-invariant test.",
    )
    .assert_ok("memory add");
    rig.step(&["memory", "list"], &["memory", "list", "--scope", "alpha"])
        .assert_ok("memory list");

    // --- task send / cancel, with zero live sessions anywhere in the
    // instance: guarantees `queued`/deferred, so this never depends on
    // whatever a real adapter call would do.
    let sent = rig.step(
        &["task", "send"],
        &[
            "task",
            "send",
            "--scope",
            "alpha",
            "--prompt",
            "do the thing",
            "--agent-name",
            "alpha-agent",
        ],
    );
    sent.assert_ok("task send");
    let sent_task_id = sent.json()["task_id"]
        .as_str()
        .expect("task_id in task send result")
        .to_string();
    rig.step(
        &["task", "cancel"],
        &["task", "cancel", "--task-id", &sent_task_id],
    )
    .assert_ok("task cancel");

    // --- agent stop: needs an existing session, seeded directly rather than
    // started for real (see module docs on why `agent start` is exempt).
    // `session_1` has no recorded pane and no running task, so
    // `ops::agent::stop` never calls the adapter at all (`pane::read` returns
    // `None`) and takes its plain `factory_session::stop` path — a fully
    // deterministic "stopped", not a maybe.
    let session_1 = uid(101);
    rig.fixture("seed session (running, for `agent stop`)", |db| {
        seed_session(db, session_1, alpha_id, "alpha-agent", &ws1);
    });
    let stop_agent = rig.step(
        &["agent", "stop"],
        &["agent", "stop", "--session", &session_1.to_string()],
    );
    stop_agent.assert_ok("agent stop");
    assert_eq!(stop_agent.json()["state"], "stopped");

    // --- task done / fail / block, each against a freshly seeded `running`
    // task assigned to the same session in turn: only `status = 'running'`
    // occupies a session (`tasks_one_running_per_session`), so each command
    // frees it for the next.
    let session_2 = uid(102);
    rig.fixture("seed session (running, for done/fail/block/assign)", |db| {
        seed_session(db, session_2, alpha_id, "alpha-agent", &ws2);
    });

    let task_done = uid(201);
    rig.fixture("seed task (running, for `task done`)", |db| {
        seed_task(
            db,
            task_done,
            alpha_id,
            "running",
            None,
            Some(session_2),
            "task to be done",
        );
    });
    rig.step(
        &["task", "done"],
        &[
            "task",
            "done",
            "--task-id",
            &task_done.to_string(),
            "--summary",
            "finished",
        ],
    )
    .assert_ok("task done");

    let task_fail = uid(202);
    rig.fixture("seed task (running, for `task fail`)", |db| {
        seed_task(
            db,
            task_fail,
            alpha_id,
            "running",
            None,
            Some(session_2),
            "task to be failed",
        );
    });
    rig.step(
        &["task", "fail"],
        &[
            "task",
            "fail",
            "--task-id",
            &task_fail.to_string(),
            "--summary",
            "could not finish",
        ],
    )
    .assert_ok("task fail");

    let task_block = uid(203);
    rig.fixture("seed task (running, for `task block`)", |db| {
        seed_task(
            db,
            task_block,
            alpha_id,
            "running",
            None,
            Some(session_2),
            "task to be blocked",
        );
    });
    rig.step(
        &["task", "block"],
        &[
            "task",
            "block",
            "--task-id",
            &task_block.to_string(),
            "--reason",
            "clarification",
        ],
    )
    .assert_ok("task block");

    // --- task resume: needs `blocked`/`interrupted`. Left with no assigned
    // session, matching migration 3's own precedent for exactly this state
    // (its doc comment in `factory-store/src/schema.rs` records that a
    // pre-existing `running` row with no recorded session is rewritten to
    // `blocked`/`interrupted` with `assigned_session_id` still NULL).
    let task_resume = uid(204);
    rig.fixture(
        "seed task (blocked: interrupted, for `task resume`)",
        |db| {
            seed_task(
                db,
                task_resume,
                alpha_id,
                "blocked",
                Some("interrupted"),
                None,
                "task to be resumed",
            );
        },
    );
    rig.step(
        &["task", "resume"],
        &["task", "resume", "--task-id", &task_resume.to_string()],
    )
    .assert_ok("task resume");

    // --- task assign: needs `queued`; "never delivers" per its own doc
    // comment. `session_2` is free again after `task block` above.
    let task_assign = uid(205);
    rig.fixture("seed task (queued, for `task assign`)", |db| {
        seed_task(
            db,
            task_assign,
            alpha_id,
            "queued",
            None,
            None,
            "task to be assigned",
        );
    });
    // `ops::task::assign` never calls the adapter at all — it only records an
    // assignment, or reports a deferral, and either is `h.success(...)`
    // (confirmed directly against its source) — so this always succeeds,
    // regardless of whether `session_2` happens to be idle again by now.
    rig.step(
        &["task", "assign"],
        &[
            "task",
            "assign",
            "--task-id",
            &task_assign.to_string(),
            "--agent-name",
            "alpha-agent",
        ],
    )
    .assert_ok("task assign");

    // --- task progress / decision: any existing task id will do.
    rig.step(
        &["task", "progress"],
        &[
            "task",
            "progress",
            "--task-id",
            &task_assign.to_string(),
            "--note",
            "halfway there",
        ],
    )
    .assert_ok("task progress");
    rig.step(
        &["task", "decision"],
        &[
            "task",
            "decision",
            "--task-id",
            &task_assign.to_string(),
            "--decision",
            "use approach B",
            "--rationale",
            "approach A needed a schema change we don't own",
        ],
    )
    .assert_ok("task decision");

    // --- task verify: `ops::task::verify` always authors with `None` (ADR
    // 0021 decision 5 — version 1 has no agent-initiated verification, and
    // `factory task verify` names no `--session` flag at all), so the
    // independence guard (`VerifyError::NotIndependent`) has nothing to
    // refuse here and this is a deterministic success against any task id.
    rig.step(
        &["task", "verify"],
        &[
            "task",
            "verify",
            "--task-id",
            &task_done.to_string(),
            "--verdict",
            "pass",
            "--note",
            "looks correct",
        ],
    )
    .assert_ok("task verify");

    // --- task rework: "the referenced run must be terminal" — `task_done` is.
    rig.step(
        &["task", "rework"],
        &[
            "task",
            "rework",
            "--scope",
            "alpha",
            "--prompt",
            "redo it, handling the edge case",
            "--reworks-task-id",
            &task_done.to_string(),
            "--rework-finding",
            "missed the edge case",
        ],
    )
    .assert_ok("task rework");

    // --- task list / show.
    rig.step(&["task", "list"], &["task", "list"])
        .assert_ok("task list");
    rig.step(
        &["task", "show"],
        &["task", "show", "--task-id", &task_done.to_string()],
    )
    .assert_ok("task show");

    // --- schedule create/list/enable/disable. `scope` and `schedule_id` are
    // positional, not `--scope`/`--schedule-id` flags (see `cli.rs`'s own
    // tests for this exact shape).
    let created = rig.step(
        &["schedule", "create"],
        &[
            "schedule",
            "create",
            "alpha",
            "--task",
            "Write the weekly status report.",
            "--name",
            "write-invariant-schedule",
            "--cron",
            "0 9 * * 1-5",
            "--tz",
            "UTC",
            "--agent",
            "alpha-agent",
            "--acceptance",
            "Covers every open task.",
        ],
    );
    created.assert_ok("schedule create");
    let schedule_id = created.json()["id"]
        .as_str()
        .expect("id in schedule create result")
        .to_string();
    rig.step(&["schedule", "list"], &["schedule", "list"])
        .assert_ok("schedule list");
    rig.step(
        &["schedule", "disable"],
        &["schedule", "disable", &schedule_id],
    )
    .assert_ok("schedule disable");
    rig.step(
        &["schedule", "enable"],
        &["schedule", "enable", &schedule_id],
    )
    .assert_ok("schedule enable");

    // --- context show.
    rig.step(
        &["context", "show"],
        &[
            "context",
            "show",
            "--scope",
            "alpha",
            "--agent-name",
            "alpha-agent",
        ],
    )
    .assert_ok("context show");

    // --- stop: the last command; nothing runs against this daemon after it.
    rig.step(&["stop"], &["stop"]).assert_ok("stop");
    daemon.stopped = true;

    // --- ADR 0022 decision 10's own assertion: every leaf the parser knows
    // about is either invoked above or named in `EXEMPT`. A leaf in neither
    // fails here, naming it — this is what stops a command added next year
    // from escaping the rule silently.
    let exempt_leaves: HashSet<Vec<String>> = EXEMPT.iter().map(|(parts, _)| leaf(parts)).collect();
    let unaccounted: Vec<String> = leaves
        .iter()
        .filter(|l| !rig.invoked.contains(*l) && !exempt_leaves.contains(*l))
        .map(|l| l.join(" "))
        .collect();
    assert!(
        unaccounted.is_empty(),
        "leaf(s) {unaccounted:?} are in neither the invoked script nor the exemption table in \
         write_invariant.rs — add an invocation or a recorded reason"
    );

    // The other direction: a stale entry (a renamed or removed subcommand
    // still named here) is also a bug in this file, not a pass.
    let leaves_set: HashSet<Vec<String>> = leaves.into_iter().collect();
    for l in rig.invoked.iter().chain(exempt_leaves.iter()) {
        assert!(
            leaves_set.contains(l),
            "`{}` is listed in write_invariant.rs but `Cli::command()` has no such leaf any more",
            l.join(" ")
        );
    }
}
