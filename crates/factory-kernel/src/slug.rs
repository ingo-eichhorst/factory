/// `^[a-z0-9][a-z0-9-]*$` -- a dataset's `name`, and a case's `id`.
pub fn is_slug(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_slug_validation_never_accepts_a_path_or_a_catalogue_escape() {
        for valid in ["a", "1", "a-1", "dataset"] {
            assert!(is_slug(valid));
        }
        for invalid in [
            "",
            "../outside",
            "/absolute",
            "Upper",
            "a_b",
            "-a",
            "a/b",
            "a\\b",
            "a\n",
        ] {
            assert!(!is_slug(invalid));
        }
    }
}
