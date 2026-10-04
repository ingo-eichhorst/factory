//! Enforce physical layer edges using Cargo's canonical package names, not
//! dependency aliases or a textual scan. The compatibility facade and router
//! sit outside the ladder; a level must never depend on either of them.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::process::Command;

fn violations(packages: &[Value]) -> Vec<String> {
    let workspace_names: std::collections::BTreeSet<&str> =
        packages.iter().filter_map(|p| p["name"].as_str()).collect();
    let levels: BTreeMap<&str, u64> = packages
        .iter()
        .filter_map(|p| {
            Some((
                p["name"].as_str()?,
                p["metadata"]["factory"]["level"].as_u64()?,
            ))
        })
        .collect();
    let mut errors = Vec::new();
    for package in packages {
        let name = package["name"].as_str().unwrap();
        let Some(&level) = levels.get(name) else {
            continue;
        };
        assert!(level <= 6, "{name}: invalid level {level}");
        for dependency in package["dependencies"].as_array().unwrap() {
            // Cargo exposes the original package name here even with rename,
            // target-specific tables, or dev/build dependencies.
            let target = dependency["name"].as_str().unwrap();
            if !target.starts_with("factory-") && !workspace_names.contains(target) {
                continue;
            }
            let allowed = level > 0
                && (target == "factory-kernel"
                    || levels.get(target).is_some_and(|&below| below + 1 == level));
            if !allowed {
                errors.push(format!(
                    "{name} (L{level}) -> {target} ({:?}, {:?}, alias {:?})",
                    dependency["kind"], dependency["target"], dependency["rename"]
                ));
            }
        }
    }
    errors
}

fn package(name: &str, level: Option<u64>, dependencies: Vec<Value>) -> Value {
    json!({
        "name": name, "metadata": {"factory": {"level": level}},
        "dependencies": dependencies
    })
}

fn edge(name: &str) -> Value {
    json!({"name": name, "kind": null, "target": null, "rename": null})
}

#[test]
fn physical_level_crates_depend_only_on_kernel_and_the_directly_lower_level() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let output = Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--offline",
            "--no-deps",
            "--format-version",
            "1",
        ])
        .current_dir(root)
        .output()
        .expect("run Cargo metadata");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout).unwrap();
    let packages = metadata["packages"].as_array().unwrap();
    // Losing an owner declaration must fail, not silently remove its guard.
    for (name, level) in [
        ("factory-kernel", 0),
        ("factory-infrastructure", 1),
        ("factory-environment", 2),
        ("factory-agents", 3),
        ("factory-process", 4),
        ("factory-assurance", 5),
        ("factory-direction", 6),
    ] {
        let owner = packages.iter().find(|p| p["name"] == name).unwrap();
        assert_eq!(owner["metadata"]["factory"]["level"], level);
    }
    let errors = violations(packages);
    assert!(
        errors.is_empty(),
        "forbidden physical edges:\n{}",
        errors.join("\n")
    );
}

#[test]
fn all_six_levels_allow_only_adjacent_commands_and_kernel_vocabulary() {
    let mut packages = vec![package("factory-kernel", Some(0), vec![])];
    for level in 1..=6 {
        let mut deps = vec![edge("factory-kernel")];
        if level > 1 {
            deps.push(edge(&format!("factory-level-{}", level - 1)));
        }
        packages.push(package(
            &format!("factory-level-{level}"),
            Some(level),
            deps,
        ));
    }
    assert!(violations(&packages).is_empty());
    for owner in 1..=6 {
        for target in 1..=6 {
            let mut mutated = packages.clone();
            mutated[owner]["dependencies"]
                .as_array_mut()
                .unwrap()
                .push(edge(&format!("factory-level-{target}")));
            assert_eq!(
                violations(&mutated).is_empty(),
                target + 1 == owner,
                "L{owner} -> L{target}"
            );
        }
    }
}

#[test]
fn aliases_target_tables_and_dev_build_edges_cannot_reach_outside_the_ladder() {
    for target in [
        "factory-core",
        "factory-daemon",
        "factory-plugins",
        "factory-cli",
        "factory-unknown-level",
        "innocent-workspace-bridge",
    ] {
        for kind in [Value::Null, json!("dev"), json!("build")] {
            let forbidden = json!({
                "name": target, "rename": "innocent_alias", "kind": kind,
                "target": "cfg(target_os = \"linux\")"
            });
            let packages = [
                package("factory-kernel", Some(0), vec![]),
                package("factory-infrastructure", Some(1), vec![forbidden]),
                package(target, None, vec![]),
            ];
            assert_eq!(violations(&packages).len(), 1);
        }
    }
}

#[test]
fn kernel_cannot_depend_on_any_level_or_the_compatibility_facade() {
    for target in [
        "factory-core",
        "factory-infrastructure",
        "factory-environment",
    ] {
        let packages = [
            package("factory-kernel", Some(0), vec![edge(target)]),
            package(target, Some(1), vec![]),
        ];
        assert_eq!(violations(&packages).len(), 1);
    }
}
