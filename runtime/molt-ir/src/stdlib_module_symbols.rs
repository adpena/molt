use std::collections::BTreeSet;

pub const STDLIB_MODULE_SYMBOLS_ENV: &str = "MOLT_STDLIB_MODULE_SYMBOLS";

pub fn parse_stdlib_module_symbols(raw: &str) -> Result<BTreeSet<String>, String> {
    let parsed: Vec<String> = serde_json::from_str(raw).map_err(|err| {
        format!("{STDLIB_MODULE_SYMBOLS_ENV} must be a JSON array of strings: {err}")
    })?;
    let mut out = BTreeSet::new();
    for (index, symbol) in parsed.into_iter().enumerate() {
        if symbol.is_empty() {
            return Err(format!(
                "{STDLIB_MODULE_SYMBOLS_ENV}[{index}] must not be empty"
            ));
        }
        if !symbol
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(format!(
                "{STDLIB_MODULE_SYMBOLS_ENV}[{index}] must contain only ASCII letters, digits, or underscores"
            ));
        }
        if !out.insert(symbol.clone()) {
            return Err(format!(
                "{STDLIB_MODULE_SYMBOLS_ENV}[{index}] duplicates module symbol {symbol:?}"
            ));
        }
    }
    Ok(out)
}

pub fn stdlib_module_symbols_from_env() -> Result<Option<BTreeSet<String>>, String> {
    match std::env::var(STDLIB_MODULE_SYMBOLS_ENV) {
        Ok(raw) => parse_stdlib_module_symbols(&raw).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(err) => Err(format!(
            "{STDLIB_MODULE_SYMBOLS_ENV} is not valid UTF-8: {err}"
        )),
    }
}

#[cfg(feature = "native-backend")]
pub fn stdlib_module_symbols_from_env_or_panic() -> Option<BTreeSet<String>> {
    stdlib_module_symbols_from_env().unwrap_or_else(|err| panic!("{err}"))
}

/// Source ownership is classified once, before compiler-generated partitions.
pub fn is_user_owned_symbol(
    name: &str,
    entry_module: &str,
    stdlib_module_symbols: Option<&BTreeSet<String>>,
) -> bool {
    if matches!(
        name,
        "molt_main"
            | "molt_host_init"
            | "molt_init___main__"
            | "molt_isolate_import"
            | "molt_isolate_bootstrap"
    ) || name.strip_prefix("molt_init_") == Some(entry_module)
        || name
            .strip_prefix(entry_module)
            .is_some_and(|rest| rest.starts_with("__"))
    {
        return true;
    }
    let Some(symbols) = stdlib_module_symbols else {
        return false;
    };
    if let Some(module) = name.strip_prefix("molt_init_") {
        return !symbols.contains(module);
    }
    !symbols.iter().any(|module| {
        name.strip_prefix(module.as_str())
            .is_some_and(|rest| rest.starts_with("__"))
    })
}

/// Resolve only provenance emitted by the splitter, never reserved-name syntax.
pub fn original_partition_source<'a>(
    name: &'a str,
    sources: &'a std::collections::BTreeMap<String, String>,
) -> &'a str {
    let mut current = name;
    for _ in 0..=sources.len() {
        let Some(source) = sources.get(current) else {
            return current;
        };
        current = source;
    }
    panic!("cyclic compiler partition provenance for {name:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_stdlib_module_symbols_accepts_sorted_set_authority() {
        let parsed = parse_stdlib_module_symbols(r#"["sys","copy"]"#).expect("valid symbols");

        assert_eq!(
            parsed,
            BTreeSet::from(["copy".to_string(), "sys".to_string()])
        );
    }

    #[test]
    fn parse_stdlib_module_symbols_rejects_malformed_json() {
        let err = parse_stdlib_module_symbols("not-json").expect_err("invalid symbols");

        assert!(err.contains("MOLT_STDLIB_MODULE_SYMBOLS must be a JSON array of strings"));
    }

    #[test]
    fn parse_stdlib_module_symbols_rejects_empty_symbol() {
        let err = parse_stdlib_module_symbols(r#"["sys",""]"#).expect_err("empty symbol");

        assert_eq!(err, "MOLT_STDLIB_MODULE_SYMBOLS[1] must not be empty");
    }

    #[test]
    fn parse_stdlib_module_symbols_rejects_duplicate_symbol() {
        let err = parse_stdlib_module_symbols(r#"["sys","sys"]"#).expect_err("duplicate symbol");

        assert_eq!(
            err,
            r#"MOLT_STDLIB_MODULE_SYMBOLS[1] duplicates module symbol "sys""#
        );
    }

    #[test]
    fn parse_stdlib_module_symbols_rejects_non_symbol_text() {
        let err = parse_stdlib_module_symbols(r#"["json.decoder"]"#).expect_err("bad symbol text");

        assert_eq!(
            err,
            "MOLT_STDLIB_MODULE_SYMBOLS[0] must contain only ASCII letters, digits, or underscores"
        );
    }
    #[test]
    fn ownership_uses_source_provenance_without_decoding_names() {
        let stdlib = BTreeSet::from(["json".to_string(), "sys".to_string()]);
        let sources = std::collections::BTreeMap::from([
            ("opaque_a".to_string(), "app__work".to_string()),
            ("opaque_b".to_string(), "json__encode".to_string()),
            ("opaque_c".to_string(), "opaque_a".to_string()),
        ]);
        for (name, expected) in [
            ("opaque_a", true),
            ("opaque_b", false),
            ("opaque_c", true),
            ("molt_host_init", true),
            ("molt_init_json", false),
            ("molt_init_app", true),
            ("jsonish__work", true),
            ("__molt_chunk_v1_user_name", true),
        ] {
            assert_eq!(
                is_user_owned_symbol(
                    original_partition_source(name, &sources),
                    "app",
                    Some(&stdlib)
                ),
                expected,
                "{name}"
            );
        }
        assert!(!is_user_owned_symbol(
            "__molt_chunk_v1_user_name",
            "app",
            None
        ));
    }

    #[test]
    #[should_panic(expected = "cyclic compiler partition provenance")]
    fn cyclic_partition_provenance_is_rejected() {
        let sources = std::collections::BTreeMap::from([
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "a".to_string()),
        ]);
        original_partition_source("a", &sources);
    }
}
