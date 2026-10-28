//! Runtime callable-symbol admission from the exact bytes selected by the CLI.

use crate::content_digest::bytes_to_lower_hex;
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::sync::Arc;

use crate::backend_environment::{self as backend_env, BackendEnv};

const RUNTIME_CALLABLE_SYMBOLS_ENV: &str = BackendEnv::MOLT_RUNTIME_CALLABLE_SYMBOLS.name();
const RUNTIME_CALLABLE_SYMBOLS_SHA256_ENV: &str =
    BackendEnv::MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256.name();

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

/// One immutable admission of the exact CLI-selected callable buffer.
///
/// A request shares this snapshot across modules and carried worker scopes.
/// Another request must admit its own input; unscoped one-shot callers retain
/// their original fresh-read behavior. This promises no final-link locality.
pub struct RuntimeCallableSymbols {
    symbols: Option<Arc<BTreeSet<String>>>,
}

impl std::fmt::Debug for RuntimeCallableSymbols {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeCallableSymbols")
            .field(
                "symbol_count",
                &self.symbols.as_ref().map(|symbols| symbols.len()),
            )
            .finish()
    }
}

impl RuntimeCallableSymbols {
    pub(crate) fn load(
        mut lookup: impl FnMut(BackendEnv) -> Option<OsString>,
    ) -> Result<Self, String> {
        let path = lookup(BackendEnv::MOLT_RUNTIME_CALLABLE_SYMBOLS);
        let digest = lookup(BackendEnv::MOLT_RUNTIME_CALLABLE_SYMBOLS_SHA256);
        let symbols =
            load_runtime_callable_symbols(path.as_deref(), digest.as_deref())?.map(Arc::new);
        Ok(Self { symbols })
    }

    /// Invalid selected input is a contract violation, including for optional
    /// consumers. Only a completely absent pair denotes standalone codegen.
    pub fn capture() -> Arc<Self> {
        backend_env::current_runtime_callable_symbols().unwrap_or_else(|| {
            Arc::new(
                Self::load(backend_env::var_os).unwrap_or_else(|err| {
                    panic!("native runtime callable admission failed: {err}")
                }),
            )
        })
    }

    pub fn symbols(&self) -> Option<&BTreeSet<String>> {
        self.symbols.as_deref()
    }

    pub fn shared_symbols(&self) -> Option<Arc<BTreeSet<String>>> {
        self.symbols.clone()
    }

    /// Resolver emission requires the selected callable set. The existing
    /// carve-out remains confined to molt-ir's own unit-test configuration;
    /// dependencies and production consumers fail closed without admission.
    pub fn required_symbols(&self) -> Cow<'_, BTreeSet<String>> {
        if let Some(symbols) = self.symbols() {
            return Cow::Borrowed(symbols);
        }
        #[cfg(test)]
        {
            Cow::Owned(BTreeSet::new())
        }
        #[cfg(not(test))]
        {
            panic!(
                "native backend cannot emit the per-app callable resolver without the linked runtime staticlib's callable-symbol set; the CLI must supply {} and {} before codegen",
                RUNTIME_CALLABLE_SYMBOLS_ENV, RUNTIME_CALLABLE_SYMBOLS_SHA256_ENV
            )
        }
    }
}

#[cfg(test)]
mod tests;
