//! Runtime callable-symbol set loading.

pub const RUNTIME_CALLABLE_SYMBOLS_ENV: &str = "MOLT_RUNTIME_CALLABLE_SYMBOLS";

/// Load the set of callable symbols the linked runtime staticlib defines.
///
/// The CLI extracts the `molt_*` text symbols from the runtime staticlib for the
/// active stdlib profile (micro vs full select different feature sets, so the
/// available callable set differs) and writes them newline-separated to a file,
/// passing its path in [`RUNTIME_CALLABLE_SYMBOLS_ENV`]. The per-app callable
/// resolver validates candidate names against this set so it never takes the
/// address of a symbol absent from the staticlib (an unresolvable relocation).
///
/// Returns `None` when the env var is unset or the file cannot be read. The
/// required resolver path treats that as a build-environment contract violation
/// and fails closed; only in-crate tests may intentionally use the empty set.
pub fn runtime_callable_symbols_from_env() -> Option<std::collections::BTreeSet<String>> {
    let path = std::env::var_os(RUNTIME_CALLABLE_SYMBOLS_ENV)?;
    let contents = std::fs::read_to_string(&path).ok()?;
    let set: std::collections::BTreeSet<String> = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if set.is_empty() { None } else { Some(set) }
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
             `{}` was unset or pointed at an empty/unreadable file. The CLI must \
             extract the staticlib's `molt_*` text symbols (via `nm --defined-only`) \
             and expose the path before codegen; without it the resolver would emit \
             dangling relocations against absent symbols and corrupt the binary. \
             Verify `nm`/`llvm-nm` is on PATH and the runtime staticlib built \
             successfully.",
            RUNTIME_CALLABLE_SYMBOLS_ENV
        )
    }
}
