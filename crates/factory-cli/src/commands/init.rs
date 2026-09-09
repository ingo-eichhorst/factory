//! `factory init` (design §7): the one command decision 9 says cannot be a
//! daemon operation, because it creates what a daemon needs before one can
//! run. Does its work locally, and is idempotent — running it twice against
//! the same root is safe: an existing `config.yaml` is left untouched, and
//! `factory_store::Store::open` is already idempotent by its own contract.

use std::path::Path;

use crate::exit;

pub fn run(root: &Path, name: Option<String>) -> i32 {
    if let Err(source) = std::fs::create_dir_all(root) {
        eprintln!("factory: could not create {}: {source}", root.display());
        return exit::GENERIC_ERROR;
    }

    // ADR 0009 §3b: `factory init` creates the directory, then canonicalizes
    // the real directory, then verifies the result — the one command whose
    // path need not already exist when it is named.
    let root = match factory_paths::CanonicalPath::resolve(root) {
        Ok(canonical) => canonical.into_path_buf(),
        Err(source) => {
            eprintln!("factory: {source}");
            return exit::GENERIC_ERROR;
        }
    };

    let factory_dir = root.join(".factory");
    if let Err(source) = std::fs::create_dir_all(&factory_dir) {
        eprintln!(
            "factory: could not create {}: {source}",
            factory_dir.display()
        );
        return exit::GENERIC_ERROR;
    }

    let config_path = factory_daemon::config_path(&root);
    if !config_path.exists() {
        let instance_name = name.unwrap_or_else(|| default_instance_name(&root));
        let yaml = format!(
            "version: 1\ninstance:\n  id: {}\n  name: {}\nscopes: []\n",
            crate::ids::new_id(),
            yaml_quote(&instance_name),
        );
        if let Err(source) = std::fs::write(&config_path, yaml) {
            eprintln!(
                "factory: could not write {}: {source}",
                config_path.display()
            );
            return exit::GENERIC_ERROR;
        }
    }

    // Idempotent by its own contract: creates `.factory/factory.sqlite` if
    // missing, migrates it to the schema this build understands, and
    // changes nothing on a valid existing database.
    if let Err(source) = factory_store::Store::open(&root) {
        eprintln!("factory: could not create the database: {source}");
        return exit::GENERIC_ERROR;
    }

    match factory_config::load(&config_path) {
        Ok(config) => println!(
            "factory: instance {} ({}) initialized at {}",
            config.instance.id,
            config.instance.name,
            root.display()
        ),
        // An existing config.yaml that does not parse is not this command's
        // problem to fix (that is `factory doctor`'s job) — init still
        // succeeded at what it owns: the directory and the database exist.
        Err(_) => println!("factory: instance initialized at {}", root.display()),
    }

    exit::OK
}

fn default_instance_name(root: &Path) -> String {
    root.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("factory")
        .to_string()
}

/// A minimal, correct YAML double-quoted scalar: escape only the two
/// characters that would otherwise end the string early or need escaping —
/// `\` and `"` — rather than reaching for a YAML serializer this crate has
/// no dependency on.
fn yaml_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_escapes_backslash_and_quote() {
        assert_eq!(yaml_quote("plain"), "\"plain\"");
        assert_eq!(yaml_quote("a\"b"), "\"a\\\"b\"");
        assert_eq!(yaml_quote("a\\b"), "\"a\\\\b\"");
    }
}
