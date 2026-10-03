//! Runtime callable-symbol admission from the exact bytes selected by the CLI.

use crate::content_digest::bytes_to_lower_hex;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;

pub const RUNTIME_CALLABLE_SYMBOLS_ENV: &str = "MOLT_RUNTIME_CALLABLE_SYMBOLS";
pub const RUNTIME_CALLABLE_SYMBOLS_SHA256_ENV: &str = "MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256";

fn parse_admitted_callable_symbols(
    contents: &[u8],
    expected_sha256: &str,
) -> Result<BTreeSet<String>, String> {
    if expected_sha256.len() != 64
        || !expected_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "{RUNTIME_CALLABLE_SYMBOLS_SHA256_ENV} must be a lowercase SHA256 digest"
        ));
    }
    let actual = bytes_to_lower_hex(Sha256::digest(contents).as_ref());
    if actual != expected_sha256 {
        return Err(format!(
            "runtime callable input SHA256 mismatch: expected {expected_sha256}, got {actual}"
        ));
    }
    // Decode precisely the admitted buffer. Do not reopen a mutable path after
    // verification, or infer the expected digest from the file's name.
    let contents = std::str::from_utf8(contents)
        .map_err(|err| format!("runtime callable input is not UTF-8: {err}"))?;
    let symbols: BTreeSet<String> = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if symbols.is_empty() {
        return Err("runtime callable input is empty".to_string());
    }
    Ok(symbols)
}

fn load_runtime_callable_symbols(
    path: Option<&OsStr>,
    expected_sha256: Option<&OsStr>,
) -> Result<Option<BTreeSet<String>>, String> {
    let (path, expected_sha256) = match (path, expected_sha256) {
        (None, None) => return Ok(None),
        (Some(path), Some(digest)) => (Path::new(path), digest),
        _ => {
            return Err(format!(
                "{RUNTIME_CALLABLE_SYMBOLS_ENV} and {RUNTIME_CALLABLE_SYMBOLS_SHA256_ENV} \
                 must both be supplied for native callable admission"
            ));
        }
    };
    let digest = expected_sha256.to_str().ok_or_else(|| {
        format!("{RUNTIME_CALLABLE_SYMBOLS_SHA256_ENV} must be a lowercase SHA256 digest")
    })?;
    let contents = std::fs::read(path).map_err(|err| {
        format!(
            "cannot read runtime callable input {}: {err}",
            path.display()
        )
    })?;
    parse_admitted_callable_symbols(&contents, digest).map(Some)
}

/// Load callable symbols only after admitting the exact buffer against the
/// CLI's independently supplied generation digest. Invalid selected input is a
/// contract violation even for optional callers such as LLVM; it never becomes
/// an empty set. `None` means neither input was supplied, allowing standalone
/// codegen probes that do not need any runtime callable.
pub fn runtime_callable_symbols_from_env() -> Option<BTreeSet<String>> {
    let path = std::env::var_os(RUNTIME_CALLABLE_SYMBOLS_ENV);
    let digest = std::env::var_os(RUNTIME_CALLABLE_SYMBOLS_SHA256_ENV);
    load_runtime_callable_symbols(path.as_deref(), digest.as_deref())
        .unwrap_or_else(|err| panic!("native runtime callable admission failed: {err}"))
}

/// Obtain the linked runtime staticlib's callable-symbol set, failing the build
/// CLOSED when it is unavailable.
///
/// The per-app callable resolver address-takes every manifest callable via a
/// pointer relocation resolved against the staticlib. Filtering the manifest by
/// exact membership in this set is the only sound way to guarantee the resolver
/// never references a symbol the linker cannot satisfy. There is no safe
/// heuristic substitute: a `molt_`-prefixed name can be feature-gated out of the
/// active stdlib profile, so guessing re-creates dangling relocations. The CLI
/// always extracts and exposes this set before native codegen for any binary
/// that emits the resolver, so absence here is a build-environment contract
/// violation, not a recoverable condition.
///
/// `cfg(test)` is the sole carve-out: in-crate codegen unit tests call `compile`
/// directly to inspect the emitted object, but that object is never linked into
/// a final binary and no symbol file is staged for it. There, the precondition
/// does not apply, so the symbol set is empty and the resolver emits its
/// zero-entry "always not found" form with no relocations.
pub fn runtime_callable_symbols_required() -> std::collections::BTreeSet<String> {
    if let Some(symbols) = runtime_callable_symbols_from_env() {
        return symbols;
    }
    #[cfg(test)]
    {
        std::collections::BTreeSet::new()
    }
    #[cfg(not(test))]
    {
        panic!(
            "native backend cannot emit the per-app callable resolver without the \
             linked runtime staticlib's callable-symbol set. \
             `{}` and `{}` were unset. The CLI must \
             extract the staticlib's `molt_*` text symbols (via `nm --defined-only`) \
             and expose the path and SHA256 before codegen; without it the resolver would emit \
             dangling relocations against absent symbols and corrupt the binary. \
             Verify `nm`/`llvm-nm` is on PATH and the runtime staticlib built \
             successfully.",
            RUNTIME_CALLABLE_SYMBOLS_ENV, RUNTIME_CALLABLE_SYMBOLS_SHA256_ENV
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Independent SHA256 vectors for the literal bytes, not this loader's hash.
    const SYMBOLS: &[u8] = b"molt_init_sys\nmolt_main\n";
    const DIGEST: &str = "cd85e5b7627ffa5a6821fde44e31407d23a4d3da80a6e07a5b675af8d0925784";

    #[test]
    fn admits_exact_bytes_before_parsing() {
        let expected = BTreeSet::from(["molt_init_sys".to_string(), "molt_main".to_string()]);
        assert_eq!(
            parse_admitted_callable_symbols(SYMBOLS, DIGEST).unwrap(),
            expected
        );
        // Same parsed symbols are insufficient: even the newline is bound.
        assert!(
            parse_admitted_callable_symbols(&SYMBOLS[..SYMBOLS.len() - 1], DIGEST)
                .unwrap_err()
                .contains("SHA256 mismatch")
        );
        assert!(
            parse_admitted_callable_symbols(b"molt_changed\nmolt_main\n", DIGEST)
                .unwrap_err()
                .contains("SHA256 mismatch")
        );
    }

    #[test]
    fn rejects_incomplete_or_malformed_admission() {
        assert!(load_runtime_callable_symbols(None, None).unwrap().is_none());
        assert!(load_runtime_callable_symbols(Some(OsStr::new("runtime.txt")), None).is_err());
        assert!(load_runtime_callable_symbols(None, Some(OsStr::new(DIGEST))).is_err());
        for digest in [
            String::new(),
            "abc".to_string(),
            "g".repeat(64),
            DIGEST.to_uppercase(),
        ] {
            assert!(
                parse_admitted_callable_symbols(SYMBOLS, &digest)
                    .unwrap_err()
                    .contains("lowercase SHA256")
            );
        }
    }

    #[test]
    fn rejects_verified_empty_or_invalid_utf8_input() {
        assert!(
            parse_admitted_callable_symbols(
                b"",
                "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
            )
            .unwrap_err()
            .contains("empty")
        );
        assert!(
            parse_admitted_callable_symbols(
                b"\xff",
                "a8100ae6aa1940d0b663bb31cd466142ebbdbd5187131b92d93818987832eb89"
            )
            .unwrap_err()
            .contains("UTF-8")
        );
    }

    #[test]
    fn selected_path_never_falls_back_after_replacement_or_removal() {
        let path = std::env::temp_dir().join(format!(
            "molt-ir-callable-admission-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, SYMBOLS).unwrap();
        assert!(
            load_runtime_callable_symbols(Some(path.as_os_str()), Some(OsStr::new(DIGEST)))
                .unwrap()
                .unwrap()
                .contains("molt_main")
        );
        std::fs::write(&path, b"molt_changed\n").unwrap();
        assert!(
            load_runtime_callable_symbols(Some(path.as_os_str()), Some(OsStr::new(DIGEST)))
                .unwrap_err()
                .contains("SHA256 mismatch")
        );
        std::fs::remove_file(&path).unwrap();
        assert!(
            load_runtime_callable_symbols(Some(path.as_os_str()), Some(OsStr::new(DIGEST)))
                .unwrap_err()
                .contains("cannot read")
        );
    }
}
