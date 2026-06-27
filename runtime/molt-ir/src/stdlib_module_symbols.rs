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
}
