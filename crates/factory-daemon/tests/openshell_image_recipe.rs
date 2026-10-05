//! Offline recipe contracts, not proof of a working Linux compiler. The real
//! cross-build is a separate acceptance check. These stubs exercise the actual
//! image script, digest gate, host selection and scoped cc/ar wrappers (#272).
#![cfg(unix)]

use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Fixture {
    root: PathBuf,
    sdk_digest: String,
}

impl Fixture {
    fn new() -> Self {
        // Quoting in generated compiler wrappers must survive these characters.
        let root =
            std::env::temp_dir().join(format!("factory-image ' $ space-{}", uuid::Uuid::new_v4()));
        for directory in ["bin", "sdk/zig-fixture", "tmp"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        executable(
            &root.join("sdk/zig-fixture/zig"),
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$FIXTURE_ROOT/compiler.log"
case "$1" in cc) [ "$2" = -target ] && [ "$3" = aarch64-linux-musl ] ;; ar) ;; *) exit 1 ;; esac
"#,
        );
        assert!(Command::new("tar")
            .arg("-cf")
            .arg(root.join("sdk.tar"))
            .arg("-C")
            .arg(root.join("sdk"))
            .arg("zig-fixture")
            .status()
            .unwrap()
            .success());
        let sdk_digest = format!(
            "{:x}",
            Sha256::digest(std::fs::read(root.join("sdk.tar")).unwrap())
        );
        executable(
            &root.join("bin/uname"),
            r#"#!/bin/sh
case "$1" in -s) echo "$FIXTURE_OS" ;; -m) echo "$FIXTURE_ARCH" ;; *) exit 1 ;; esac
"#,
        );
        executable(
            &root.join("bin/curl"),
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$FIXTURE_ROOT/downloads.log"
while [ "$#" -gt 0 ]; do
  case "$1" in -o) output=$2; shift ;; https://*) url=$1 ;; esac
  shift
done
case "$url" in
  https://ziglang.org/*) cp "$FIXTURE_ROOT/sdk.tar" "$output" ;;
  https://github.com/herdrdev/*) printf herdr > "$output" ;;
  https://github.com/jqlang/*) printf jq > "$output" ;;
  *) exit 1 ;;
esac
"#,
        );
        executable(
            &root.join("bin/rustup"),
            "#!/bin/sh\n[ \"$*\" = 'target add aarch64-unknown-linux-musl' ]\n",
        );
        executable(
            &root.join("bin/cargo"),
            r#"#!/bin/sh
set -eu
[ "$CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER" = rust-lld ]
[ "$ZIG_LOCAL_CACHE_DIR" != "$ZIG_GLOBAL_CACHE_DIR" ]
"$CC_aarch64_unknown_linux_musl" --target=aarch64-unknown-linux-musl -c sqlite3.c -o sqlite3.o
"$AR_aarch64_unknown_linux_musl" crs libsqlite3.a sqlite3.o
printf '%s\n' "$*" > "$FIXTURE_ROOT/cargo.log"
mkdir -p "$FACTORY_TARGET_DIR/aarch64-unknown-linux-musl/release"
printf factory > "$FACTORY_TARGET_DIR/aarch64-unknown-linux-musl/release/factory"
"#,
        );
        executable(
            &root.join("bin/docker"),
            r#"#!/bin/sh
set -eu
# Compiler overrides must not leak past the cargo invocation.
[ "${CC_aarch64_unknown_linux_musl-unset}" = unset ]
[ "${AR_aarch64_unknown_linux_musl-unset}" = unset ]
[ "${FACTORY_IMAGE_ZIG_BIN-unset}" = unset ]
for stage do :; done
[ "$(cat "$stage/usr/local/bin/factory")" = factory ]
[ -s "$stage/etc/claude-code/managed-settings.json" ]
[ -s "$stage/sandbox/.claude.json" ]
printf '%s\n' "$*" > "$FIXTURE_ROOT/docker.log"
"#,
        );
        Self { root, sdk_digest }
    }

    fn run(&self, os: &str, arch: &str, overrides: &[(&str, Option<&str>)]) -> Output {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut command = Command::new("/bin/sh");
        command
            .arg(repo.join("examples/openshell/build-image.sh"))
            .arg("docker")
            .env_clear()
            .env(
                "PATH",
                format!(
                    "{}:/usr/bin:/bin:/usr/sbin:/sbin",
                    self.root.join("bin").display()
                ),
            )
            .env("TMPDIR", self.root.join("tmp"))
            .env("OUT", self.root.join("image"))
            .env("FACTORY_SOURCE", repo)
            .env("FACTORY_TARGET_DIR", self.root.join("target"))
            .env("FIXTURE_ROOT", &self.root)
            .env("FIXTURE_OS", os)
            .env("FIXTURE_ARCH", arch)
            .env("GIT_USER_NAME", "Image QA")
            .env("GIT_USER_EMAIL", "qa@example.invalid")
            .env("ZIG_SHA256", &self.sdk_digest)
            .env("HERDR_SHA256", format!("{:x}", Sha256::digest(b"herdr")))
            .env("JQ_SHA256", format!("{:x}", Sha256::digest(b"jq")));
        for (key, value) in overrides {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        command.output().unwrap()
    }

    fn log(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn supported_hosts_select_sdk_and_scope_compiler_wrappers_to_cargo() {
    for (os, arch, host) in [
        ("Darwin", "arm64", "aarch64-macos"),
        ("Darwin", "x86_64", "x86_64-macos"),
        ("Linux", "aarch64", "aarch64-linux"),
        ("Linux", "x86_64", "x86_64-linux"),
    ] {
        let fixture = Fixture::new();
        let output = fixture.run(os, arch, &[]);
        assert!(
            output.status.success(),
            "{os}/{arch}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(fixture.log("downloads.log").contains(&format!(
            "https://ziglang.org/download/0.14.1/zig-{host}-0.14.1.tar.xz"
        )));
        assert_eq!(fixture.log("compiler.log"), "cc -target aarch64-linux-musl -c sqlite3.c -o sqlite3.o\nar crs libsqlite3.a sqlite3.o\n");
        assert!(fixture
            .log("cargo.log")
            .contains("build --release -p factory-cli --target aarch64-unknown-linux-musl"));
        assert!(fixture.log("docker.log").contains("--platform linux/arm64"));
        assert_eq!(
            std::fs::read_dir(fixture.root.join("tmp")).unwrap().count(),
            0
        );
    }
}

#[test]
fn mismatched_sdk_digest_refuses_before_extraction_or_compilation() {
    let fixture = Fixture::new();
    let output = fixture.run("Darwin", "arm64", &[("ZIG_SHA256", Some("wrong"))]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("SHA-256 mismatch"));
    assert!(!fixture.root.join("compiler.log").exists());
    assert!(!fixture.root.join("cargo.log").exists());
    assert_eq!(
        std::fs::read_dir(fixture.root.join("tmp")).unwrap().count(),
        0
    );
}

#[test]
fn custom_sdk_requires_explicit_digest_and_uses_requested_version() {
    let refused = Fixture::new();
    let output = refused.run(
        "Linux",
        "x86_64",
        &[("ZIG_VERSION", Some("custom")), ("ZIG_SHA256", None)],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ZIG_SHA256 is required"));
    assert!(!refused.root.join("downloads.log").exists());
    let allowed = Fixture::new();
    let output = allowed.run("Linux", "x86_64", &[("ZIG_VERSION", Some("custom"))]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(allowed
        .log("downloads.log")
        .contains("/custom/zig-x86_64-linux-custom.tar.xz"));
}

#[test]
fn unsupported_host_refuses_without_downloading_or_building() {
    let fixture = Fixture::new();
    let output = fixture.run("Linux", "riscv64", &[]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("supports arm64/x86_64 macOS and Linux")
    );
    assert!(!fixture.root.join("downloads.log").exists());
    assert!(!fixture.root.join("cargo.log").exists());
}
