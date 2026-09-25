//! `scripts/repair-harness` (#131), run against a fake Homebrew Caskroom.
//! Nothing here touches an installed harness: the harness is a shell script
//! in a temporary directory, found through a `PATH` this test sets.
//!
//! The fake's "stall" is an extended attribute on it: it hangs while the
//! attribute is there, and the script's fresh copy (`cp -X`) does not carry
//! it -- the same shape as the incident, where the same bytes in a fresh
//! folder started at once. The signature is a stub `codesign` on `PATH`, so
//! the script's own decisions -- refuse on a bad signature, refuse on the
//! wrong team -- are what is tested; one case uses the real `codesign` to
//! show an unsigned folder is refused by the real thing too.
#![cfg(target_os = "macos")]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const STUCK_ATTR: &str = "com.factory.test.stuck";

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/repair-harness")
}

fn write_exe(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Fixture {
    dir: PathBuf,
    /// `<dir>/Caskroom/codex/0.157.0`
    folder: PathBuf,
    /// Where `codex` is found on PATH: a symlink into the folder.
    bin_dir: PathBuf,
    /// The stub `codesign`, first on PATH when used.
    stub_dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("factory-repair-{name}-{}", uuid::Uuid::new_v4()));
        let folder = dir.join("Caskroom/codex/0.157.0");
        let binary = folder.join("bin/codex");
        write_exe(
            &binary,
            &format!(
                "#!/bin/sh\nif xattr -p {STUCK_ATTR} \"$0\" >/dev/null 2>&1; then sleep 30; fi\necho 'codex-cli 0.157.0'\n"
            ),
        );
        std::fs::write(folder.join("codex-package.json"), "{}").unwrap();
        let bin_dir = dir.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::os::unix::fs::symlink(&binary, bin_dir.join("codex")).unwrap();
        let stub_dir = dir.join("stub");
        write_exe(
            &stub_dir.join("codesign"),
            "#!/bin/sh\n\
             case \"$1\" in\n\
               --verify) exit \"${FAKE_SIGN_STATUS:-0}\" ;;\n\
               -dv) echo \"Identifier=codex\" >&2; echo \"TeamIdentifier=${FAKE_TEAM:-2DC432GLL2}\" >&2; exit 0 ;;\n\
             esac\n\
             exit 1\n",
        );
        Self { dir, folder, bin_dir, stub_dir }
    }

    fn stick(&self) {
        let status = Command::new("xattr")
            .args(["-w", STUCK_ATTR, "1"])
            .arg(self.folder.join("bin/codex"))
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn run(&self, args: &[&str], stub: bool, env: &[(&str, &str)]) -> Output {
        let mut path = String::new();
        if stub {
            path.push_str(&format!("{}:", self.stub_dir.display()));
        }
        path.push_str(&format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", self.bin_dir.display()));
        let mut cmd = Command::new("/bin/bash");
        cmd.arg(script())
            .args(args)
            .env_clear()
            .env("PATH", path)
            .env("HOME", &self.dir)
            .env("TMPDIR", &self.dir)
            .env("REPAIR_HARNESS_TIMEOUT", "5");
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    /// Every name in the cask's folder, sorted.
    fn versions(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.folder.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
fn a_stuck_install_is_swapped_for_a_fresh_copy_and_the_old_one_is_kept() {
    let f = Fixture::new("swap");
    f.stick();
    let out = f.run(&["codex"], true, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("repaired"), "{}", text(&out));

    let versions = f.versions();
    assert_eq!(versions, vec!["0.157.0", "0.157.0.stuck"], "{versions:?}");
    // The one in place answers; the kept one still carries the stall and
    // everything it had -- nothing was deleted.
    let fresh = Command::new(f.bin_dir.join("codex")).arg("--version").output().unwrap();
    assert!(fresh.status.success());
    let kept = f.folder.with_file_name("0.157.0.stuck");
    assert!(kept.join("codex-package.json").exists());
    let attr = Command::new("xattr").args(["-p", STUCK_ATTR]).arg(kept.join("bin/codex")).output().unwrap();
    assert!(attr.status.success(), "the old folder is kept exactly as it was");
}

#[test]
fn a_second_repair_keeps_the_first_stuck_folder_too() {
    let f = Fixture::new("twice");
    std::fs::create_dir_all(f.folder.with_file_name("0.157.0.stuck")).unwrap();
    f.stick();
    let out = f.run(&["codex"], true, &[]);
    assert!(out.status.success(), "{}", text(&out));
    let versions = f.versions();
    assert_eq!(versions.len(), 3, "{versions:?}");
    assert!(versions.iter().any(|v| v.starts_with("0.157.0.stuck.")), "{versions:?}");
}

#[test]
fn the_wrong_developer_team_is_refused_and_nothing_changes() {
    let f = Fixture::new("team");
    f.stick();
    let out = f.run(&["codex"], true, &[("FAKE_TEAM", "ABCDE12345")]);
    assert!(!out.status.success());
    let said = text(&out);
    assert!(said.contains("refusing") && said.contains("ABCDE12345") && said.contains("2DC432GLL2"), "{said}");
    assert_eq!(f.versions(), vec!["0.157.0"]);
}

#[test]
fn a_signature_that_does_not_verify_is_refused_and_nothing_changes() {
    let f = Fixture::new("sig");
    f.stick();
    let out = f.run(&["codex"], true, &[("FAKE_SIGN_STATUS", "1")]);
    assert!(!out.status.success());
    assert!(text(&out).contains("does not verify"), "{}", text(&out));
    assert_eq!(f.versions(), vec!["0.157.0"]);
}

#[test]
fn the_real_codesign_refuses_an_unsigned_folder() {
    let f = Fixture::new("real");
    f.stick();
    let out = f.run(&["codex"], false, &[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("does not verify"), "{}", text(&out));
    assert_eq!(f.versions(), vec!["0.157.0"]);
}

#[test]
fn a_harness_that_already_answers_is_left_alone() {
    let f = Fixture::new("fine");
    let out = f.run(&["codex"], true, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("nothing to repair"), "{}", text(&out));
    assert_eq!(f.versions(), vec!["0.157.0"]);
}

#[test]
fn a_dry_run_changes_nothing() {
    let f = Fixture::new("dry");
    f.stick();
    let out = f.run(&["--dry-run", "codex"], true, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(text(&out).contains("dry run"), "{}", text(&out));
    assert_eq!(f.versions(), vec!["0.157.0"]);
}

#[test]
fn a_harness_with_no_known_team_or_no_cask_is_refused() {
    let f = Fixture::new("unknown");
    let out = f.run(&["opencode"], true, &[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("no expected developer team"), "{}", text(&out));

    // `claude` here is not in a Caskroom: Claude Code's own installer.
    write_exe(&f.dir.join("share/claude/versions/2.1.282"), "#!/bin/sh\necho 2.1.282\n");
    std::os::unix::fs::symlink(f.dir.join("share/claude/versions/2.1.282"), f.bin_dir.join("claude")).unwrap();
    let out = f.run(&["claude"], true, &[]);
    assert!(!out.status.success());
    assert!(text(&out).contains("not inside a Homebrew cask"), "{}", text(&out));
}
