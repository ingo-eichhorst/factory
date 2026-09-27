import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  CONFIG_SNIPPET,
  MISSING,
  actions,
  ageBadge,
  backupFailure,
  checkRows,
  codeRows,
  destinationLevel,
  destinationText,
  fmtAgo,
  fmtIn,
  fmtSpan,
  fmtWhen,
  identityHint,
  includeCount,
  isBackupEvent,
  keepText,
  keptByText,
  lastVerifiedText,
  rowVerifyState,
  scheduleText,
  timeMachineText,
  verifiedCell,
} from "../js/backup-model.js";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const view = readFileSync(new URL("../js/backup.js", import.meta.url), "utf8");
const served = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");

// The wire shape of `GET /api/backup`'s report (`backup::BackupReport`),
// as the daemon answered it on a throwaway instance: one snapshot taken
// and verified, a destination on the same disk.
const REPORT = {
  now: "2026-09-25T12:00:00Z",
  config: {
    destination: "/tmp/dev-backup-116-dest",
    schedule: { cron: "0 3 * * *", timezone: "Europe/Berlin" },
    keep: { daily: 7, weekly: 4, monthly: 6 },
  },
  destination: {
    path: "/tmp/dev-backup-116-dest",
    exists: true,
    same_device: true,
    free_bytes: 1539316654080,
    total_bytes: 1995218165760,
  },
  age: "fresh",
  due_by: "2026-09-26T03:00:00Z",
  next_run: "2026-09-26T01:00:00Z",
  running: false,
  last_verified: { snapshot: "factory-backup-dev-20260925T090000Z.tar.zst", at: "2026-09-25T09:05:00Z", ok: true },
  last_failure: null,
  warnings: [
    {
      kind: "same_device",
      level: "bad",
      message: "The destination /tmp/dev-backup-116-dest is on the same device as the instance, so a disk failure loses both: this is a copy, not a backup.",
    },
  ],
  snapshots: [
    {
      name: "factory-backup-dev-20260925T090000Z.tar.zst",
      at: "2026-09-25T09:00:00Z",
      size_bytes: 48213,
      files: 7,
      verified: { snapshot: "factory-backup-dev-20260925T090000Z.tar.zst", at: "2026-09-25T09:05:00Z", ok: true },
      kept_by: ["newest", "daily", "weekly", "monthly"],
      encrypted: false,
    },
  ],
  include: [
    { path: ".factory/factory.sqlite", why: "tasks, runs…", included: true, files: 1, bytes: 40960 },
    { path: ".factory/logs/", why: "the daemon's logs", included: false, files: null, bytes: null },
  ],
  exclude: [{ path: ".factory/secrets.yaml", why: "a secret: never read, never copied" }],
  // #155: the wire shape `RepositoryFact`/`TimeMachineFact` serialize to --
  // pinned by a Rust test (`a_repository_fact_and_a_time_machine_fact_serialize_the_way_the_ui_expects`).
  code: [
    {
      scopes: ["factory"],
      path: "projects/factory",
      state: { state: "tracked", remote: "origin", upstream: "origin/main", ahead: 2 },
      remote_url: "https://github.com/o/factory.git",
    },
    {
      scopes: ["root", "nested"],
      path: ".",
      state: { state: "tracked", remote: "origin", upstream: "origin/main", ahead: 0 },
      remote_url: "https://github.com/o/root.git",
    },
    { scopes: ["no-remote"], path: "projects/no-remote", state: { state: "no_remote" }, remote_url: null },
    { scopes: ["no-upstream"], path: "projects/no-upstream", state: { state: "no_upstream" }, remote_url: null },
    { scopes: ["detached"], path: "projects/detached", state: { state: "detached_head" }, remote_url: null },
    { scopes: ["fresh"], path: "projects/fresh", state: { state: "no_commits" }, remote_url: null },
    { scopes: ["not-a-repo"], path: "projects/not-a-repo", state: { state: "not_a_repository" }, remote_url: null },
    { scopes: ["gone"], path: "gone", state: { state: "no_directory" }, remote_url: null },
    {
      scopes: ["flaky"],
      path: "projects/flaky",
      state: { state: "inspection_failed", reason: "git timed out after 5s" },
      remote_url: null,
    },
  ],
  time_machine: { state: "not_configured" },
};

// ------------------------------------------------------------------ the tab

test("L1 gains a Backup tab after Infrastructure, with a view and a sub-label that names it", () => {
  assert.match(page, /id="tab-infrastructure"[^>]*>Infrastructure<\/button>[\s\S]*<button id="tab-backup" hidden>Backup<\/button>/);
  assert.match(page, /id="view-backup"/);
  assert.match(page, /<span class="lv-sub">Host, daemon, Doctor and backup<\/span>/);
  assert.match(app, /infra: \["infrastructure", "doctor", "backup"\]/);
  assert.match(app, /backup: \{ onShow: startBackup, onHide: stopAgentPoll \}/);
  assert.match(app, /state\.tab === "backup"/, "the rail's re-render names every tab");
  assert.match(app, /isBackupEvent\(ev\) && state\.tab === "backup"/, "backup_* events refetch the page");
});

test("both modules are served, or the browser 404s them", () => {
  assert.match(served, /"js\/backup\.js"/);
  assert.match(served, /"js\/backup-model\.js"/);
});

test("the two buttons start disabled, and nothing on the page asks the browser to confirm", () => {
  assert.match(page, /id="backup-verify" disabled>Verify newest</);
  assert.match(page, /id="backup-run" disabled>Back up now</);
  assert.doesNotMatch(view, /\b(alert|confirm|prompt)\(/, "confirmation is a modal.js scrim");
  assert.match(view, /from "\.\/modal\.js"/);
});

test("a daemon without the endpoint gets a calm note, apart from the error", () => {
  assert.match(page, /id="backup-unavailable"[^>]*hidden/);
  assert.match(page, /id="backup-error"[^>]*hidden/);
  assert.equal(backupFailure(new Error("404 Not Found")), "unavailable");
  assert.equal(backupFailure(new Error("500 Internal Server Error")), "error");
  assert.equal(backupFailure(undefined), "error");
});

// ------------------------------------------------------------------ the hero

test("the age badge is green, amber or red as the daemon decided -- never recomputed here", () => {
  assert.deepEqual(ageBadge(REPORT), { level: "ok", label: "Fresh" });
  assert.equal(ageBadge({ ...REPORT, age: "stale" }).level, "warn");
  assert.equal(ageBadge({ ...REPORT, age: "overdue" }).level, "bad");
  assert.deepEqual(ageBadge({ ...REPORT, age: "none" }), { level: "bad", label: "No backup yet" });
  assert.deepEqual(ageBadge({ ...REPORT, config: null }), { level: "bad", label: "Not configured" });
  assert.deepEqual(ageBadge(null), { level: "bad", label: "Not configured" });
});

test("ages read against the report's own clock", () => {
  assert.equal(fmtAgo(REPORT.now, REPORT.snapshots[0].at), "3h 0m ago");
  assert.equal(fmtIn(REPORT.now, REPORT.next_run), "in 13h 0m");
  assert.equal(fmtIn(REPORT.now, "2026-09-25T11:59:30Z"), "now", "a due slot is now, not a negative span");
  assert.equal(fmtAgo(REPORT.now, null), MISSING);
  assert.equal(fmtSpan(42), "42s");
  assert.equal(fmtSpan(3 * 86400 + 4 * 3600), "3d 4h");
  assert.equal(fmtWhen("2026-09-25T09:00:00Z"), "2026-09-25 09:00 UTC");
  assert.equal(fmtWhen(null), MISSING);
});

test("a destination on the same device is red and says so; another device is green", () => {
  assert.equal(destinationText(REPORT.destination), "same device as the instance · 1.4 TB free");
  assert.equal(destinationLevel(REPORT.destination), "bad");
  const off = { ...REPORT.destination, same_device: false, free_bytes: null };
  assert.equal(destinationText(off), "another device");
  assert.equal(destinationLevel(off), "ok");
  const missing = { path: "/Volumes/Backup", exists: false, same_device: null };
  assert.match(destinationText(missing), /^missing/);
  assert.equal(destinationLevel(missing), "bad");
  assert.equal(destinationLevel({ ...off, same_device: null }), "warn", "unknown is not claimed either way");
});

test("the schedule, retention and last verification read as a person would say them", () => {
  assert.equal(scheduleText(REPORT.config), "0 3 * * * (Europe/Berlin)");
  assert.equal(scheduleText({ ...REPORT.config, schedule: { cron: "0 3 * * *" } }), "0 3 * * * (UTC)");
  assert.match(scheduleText({ destination: "/b", keep: REPORT.config.keep }), /^none/);
  assert.equal(keepText(REPORT.config), "7 daily · 4 weekly · 6 monthly");
  assert.equal(lastVerifiedText(REPORT), "passed 2h 55m ago (2026-09-25 09:05 UTC)");
  assert.equal(lastVerifiedText({ ...REPORT, last_verified: null }), "never");
});

test("the config snippet is the issue's block, minimal -- encrypt_to is optional and left out", () => {
  assert.match(CONFIG_SNIPPET, /^infrastructure:\n {2}backup:/m);
  assert.match(CONFIG_SNIPPET, /keep: \{ daily: 7, weekly: 4, monthly: 6 \}/);
  assert.doesNotMatch(CONFIG_SNIPPET, /encrypt_to/);
});

// ------------------------------------------------------------------ history

test("each snapshot says whether it was verified and which rule keeps it", () => {
  const [s] = REPORT.snapshots;
  assert.deepEqual(verifiedCell(s.verified), { level: "ok", text: "verified 2026-09-25 09:05 UTC" });
  assert.equal(verifiedCell({ ...s.verified, ok: false }).level, "bad");
  assert.deepEqual(verifiedCell(null), { level: "none", text: "not verified" });
  assert.equal(keptByText(s.kept_by), "newest · daily · weekly · monthly");
  assert.equal(keptByText([]), "deleted at the next backup");
});

test("the include table: counts from the newest snapshot, unknown before one, off when optional", () => {
  assert.equal(includeCount(REPORT.include[0]), "1 file · 40 KB");
  assert.equal(includeCount(REPORT.include[1]), "off");
  assert.equal(includeCount({ ...REPORT.include[0], files: null }), MISSING);
});

test("the buttons: nothing before a config, nothing while running, no verify without a snapshot", () => {
  assert.deepEqual(actions(REPORT), { run: true, verify: true, why: "" });
  assert.equal(actions({ ...REPORT, config: null }).run, false);
  assert.equal(actions({ ...REPORT, running: true }).verify, false);
  const empty = actions({ ...REPORT, snapshots: [] });
  assert.equal(empty.run, true);
  assert.equal(empty.verify, false);
  assert.match(empty.why, /no snapshot/);
});

// -------------------------------------------------------------------- #152

test("an encrypted newest snapshot disables the hero Verify with the CLI hint, but never Back up now", () => {
  const encrypted = {
    ...REPORT,
    snapshots: [{ ...REPORT.snapshots[0], name: "factory-backup-dev-20260925T090000Z.tar.zst.age", encrypted: true }],
  };
  const can = actions(encrypted);
  assert.equal(can.run, true, "encryption never stops taking a backup");
  assert.equal(can.verify, false);
  assert.match(can.why, /--identity/);
  assert.match(can.why, /factory backup verify/);
});

test("identityHint names the CLI command for the given snapshot", () => {
  assert.equal(
    identityHint("factory-backup-dev-20260925T090000Z.tar.zst.age"),
    "encrypted: run `factory backup verify factory-backup-dev-20260925T090000Z.tar.zst.age --identity <file>`"
  );
});

test("a row's Verify is enabled for a plaintext snapshot and disabled with a hint for an encrypted one", () => {
  const plain = REPORT.snapshots[0];
  assert.deepEqual(rowVerifyState(plain), { enabled: true, title: "" });
  const encrypted = { ...plain, encrypted: true };
  const state = rowVerifyState(encrypted);
  assert.equal(state.enabled, false);
  assert.match(state.title, /--identity/);
  assert.deepEqual(rowVerifyState(null), { enabled: true, title: "" });
});

test("the hero reads the configured recipient from encrypt_to, never a hardcoded no", () => {
  assert.match(view, /config\.encrypt_to/, "the hero reads encrypt_to from the config");
  assert.doesNotMatch(view, /no -- v1 writes plaintext archives/, "the old v1-only line is gone");
});

test("history rows read encrypted state through rowVerifyState, not their own logic", () => {
  assert.match(view, /rowVerifyState/);
});

test("a verification's steps keep their order and draw a failure red", () => {
  const rows = checkRows({
    checks: [
      { name: "archive", status: "ok", detail: "unpacked 7 files" },
      { name: "policies", status: "warn", detail: "could not parse broken.yaml" },
      { name: "checksums", status: "fail", detail: "x does not match its sha256" },
    ],
  });
  assert.deepEqual(rows.map(r => [r.name, r.level, r.status]), [
    ["archive", "ok", "ok"],
    ["policies", "warn", "warn"],
    ["checksums", "bad", "FAIL"],
  ]);
  assert.deepEqual(checkRows(null), []);
});

test("only backup_* events are the page's", () => {
  assert.equal(isBackupEvent({ type: "backup_completed" }), true);
  assert.equal(isBackupEvent({ type: "backup_failed" }), true);
  assert.equal(isBackupEvent({ type: "backup_verified" }), true);
  assert.equal(isBackupEvent({ type: "task_updated" }), false);
  assert.equal(isBackupEvent(null), false);
});

// -------------------------------------------------------------------- #155

test("code is a row per scope, sharing one repository's state and remote", () => {
  const rows = codeRows(REPORT);
  // The 9 facts above cover 10 scope rows: one fact lists two scopes.
  assert.equal(rows.length, 10);
  const factory = rows.find(r => r.scope === "factory");
  assert.deepEqual(factory, { scope: "factory", remote: "https://github.com/o/factory.git", level: "warn", text: "2 unpushed (as of last fetch)" });
  const shared = rows.filter(r => r.scope === "root" || r.scope === "nested");
  assert.equal(shared.length, 2);
  for (const row of shared) {
    assert.equal(row.level, "ok");
    assert.equal(row.text, "up to date (as of last fetch)");
    assert.equal(row.remote, "https://github.com/o/root.git");
  }
});

test("an unpushed commit is a warning; up to date and unknown states are not", () => {
  assert.equal(codeRows({ code: [{ scopes: ["a"], state: { state: "tracked", ahead: 1 } }] })[0].level, "warn");
  assert.equal(codeRows({ code: [{ scopes: ["a"], state: { state: "tracked", ahead: 0 } }] })[0].level, "ok");
  assert.equal(codeRows({ code: [{ scopes: ["a"], state: { state: "no_remote" } }] })[0].level, "warn");
  assert.equal(codeRows({ code: [{ scopes: ["a"], state: { state: "no_upstream" } }] })[0].level, "warn");
  // Never shown as a problem or as fine: unknown, exactly as the daemon warns.
  for (const state of ["detached_head", "no_commits", "not_a_repository", "no_directory", "inspection_failed"]) {
    assert.equal(codeRows({ code: [{ scopes: ["a"], state: { state } }] })[0].level, "none", state);
  }
  assert.deepEqual(codeRows({ code: [] }), []);
  assert.deepEqual(codeRows({}), []);
  assert.deepEqual(codeRows(null), []);
});

test("no scope name reads as the missing dash instead of an empty row", () => {
  const rows = codeRows({ code: [{ scopes: [], state: { state: "no_remote" } }] });
  assert.equal(rows[0].scope, MISSING);
});

test("time machine reads observed, unknown or unsupported -- never asserted from nothing", () => {
  assert.deepEqual(timeMachineText({ time_machine: { state: "configured", destinations: ["Backup Disk"] } }), {
    level: "ok",
    text: "configured -- Backup Disk",
  });
  assert.equal(timeMachineText(REPORT).level, "warn");
  assert.equal(timeMachineText(REPORT).text, "not configured");
  const unavailable = timeMachineText({ time_machine: { state: "unavailable", reason: "tmutil is missing" } });
  assert.equal(unavailable.level, "none");
  assert.match(unavailable.text, /tmutil is missing/);
  assert.deepEqual(timeMachineText({ time_machine: { state: "unsupported" } }), { level: "none", text: "not supported on this platform" });
  assert.deepEqual(timeMachineText({}), { level: "none", text: "unknown" });
  assert.deepEqual(timeMachineText(null), { level: "none", text: "unknown" });
});

test("the Code section renders for every report, not gated on report.config the way history and contents are", () => {
  assert.match(view, /function codeSection\(report\)/);
  assert.match(
    view,
    /\[hero\(report\), warningStrip\(report\.warnings\), codeSection\(report\), history\(report\), contents\(report\)\]/
  );
});
