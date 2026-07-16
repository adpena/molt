//! Pure string-intern admission facts.
//!
//! Stable storage, winner publication, and teardown are owned by
//! `CanonicalObjectCache`; this module deliberately has no pool or lifetime
//! authority.

/// Returns `true` when `s` is an ASCII Python identifier:
/// `[a-zA-Z_][a-zA-Z0-9_]*`.
#[inline]
pub(crate) fn is_identifier_like(s: &str) -> bool {
    let Some((&first, rest)) = s.as_bytes().split_first() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && rest
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::is_identifier_like;

    #[test]
    fn identifier_classifier_is_exact_for_its_ascii_domain() {
        for accepted in [
            "x",
            "_",
            "__init__",
            "CamelCase",
            "snake_case_123",
            "A1B2C3",
            "_private",
        ] {
            assert!(is_identifier_like(accepted), "{accepted:?}");
        }
        for rejected in ["", "1abc", "hello world", "foo-bar", "3.14", "a.b", "café"] {
            assert!(!is_identifier_like(rejected), "{rejected:?}");
        }
    }
}
