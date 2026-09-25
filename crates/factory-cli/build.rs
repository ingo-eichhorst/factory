use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=FACTORY_GIT_SHA");
    let sha = std::env::var("FACTORY_GIT_SHA").ok().or_else(|| {
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(std::env::var("CARGO_MANIFEST_DIR").ok()?)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|sha| sha.trim().to_string())
    });
    println!(
        "cargo:rustc-env=FACTORY_GIT_SHA={}",
        sha.as_deref().unwrap_or("unknown")
    );
}
