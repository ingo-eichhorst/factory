//! Guards phase 1 of #193: `factory-kernel` is L0 -- everything else in the
//! workspace may depend on it, and it depends on nothing in the workspace.
//! A `factory-*` path dependency added here would let something above L0
//! reach back down through the kernel to the very thing it names, defeating
//! the whole point of having a bottom layer nothing else can be pulled into.
//!
//! No `toml` crate is a workspace dependency, and this guard is not worth
//! adding one for: a line scan over the manifest's own `key = value` pairs
//! is enough to catch a dependency table entry, whichever table it is in
//! (`[dependencies]`, `[dev-dependencies]`, a target-specific table, ...).

#[test]
fn the_kernel_names_no_factory_dependency_in_its_manifest() {
    let manifest_path = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let manifest = std::fs::read_to_string(manifest_path)
        .unwrap_or_else(|e| panic!("reading {manifest_path}: {e}"));

    for raw_line in manifest.lines() {
        // Strip a trailing comment before splitting on `=`, so a comment
        // that merely mentions "factory-core" cannot fail the build.
        let line = raw_line.split('#').next().unwrap_or("").trim();
        let Some((key, _value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"');
        assert!(
            !key.starts_with("factory-"),
            "{manifest_path} names {key:?} -- factory-kernel (L0) must not \
             depend on any other factory-* crate: {raw_line:?}"
        );
    }
}
