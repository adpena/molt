//! Runtime intrinsic-symbol set loading (moved verbatim from lib.rs).

/// Load the set of intrinsic symbols the linked runtime staticlib defines.
///
/// The CLI extracts the `molt_*` text symbols from the runtime staticlib for the
/// active stdlib profile (micro vs full select different feature sets, so the
/// available intrinsic set differs) and writes them newline-separated to a file,
/// passing its path in `MOLT_RUNTIME_INTRINSIC_SYMBOLS`. The per-app resolver
/// validates candidate intrinsic names against this set so it never takes the
/// address of a symbol absent from the staticlib (an unresolvable relocation).
///
/// Returns `None` when the env var is unset or the file cannot be read. The
/// required resolver path treats that as a build-environment contract violation
/// and fails closed; only in-crate tests may intentionally use the empty set.
pub fn runtime_intrinsic_symbols_from_env() -> Option<std::collections::BTreeSet<String>> {
    let path = std::env::var_os("MOLT_RUNTIME_INTRINSIC_SYMBOLS")?;
    let contents = std::fs::read_to_string(&path).ok()?;
    let set: std::collections::BTreeSet<String> = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if set.is_empty() { None } else { Some(set) }
}

/// Obtain the linked runtime staticlib's intrinsic-symbol set, failing the build
/// CLOSED when it is unavailable.
///
/// The per-app intrinsic resolver address-takes every manifest intrinsic via a
/// pointer relocation resolved against the staticlib. Filtering the manifest by
/// exact membership in this set is the only sound way to guarantee the resolver
/// never references a symbol the linker cannot satisfy — a dangling relocation
/// writes garbage into the object (historically flipping the Mach-O magic
/// `0xfeedfacf` -> `0xfeedface`, yielding a kernel-SIGKILLed binary). There is no
/// safe heuristic substitute: a `molt_`-prefixed name can be feature-gated out of
/// the active stdlib profile, so guessing re-creates that corruption. The CLI
/// always extracts and exposes this set (`MOLT_RUNTIME_INTRINSIC_SYMBOLS`) before
/// native codegen for any binary that emits the resolver, so absence here is a
/// build-environment contract violation, not a recoverable condition — panic with
/// an actionable message rather than emit a corrupt binary.
///
/// `cfg(test)` is the sole carve-out (mirroring the resolver machinery in
/// `molt-runtime`'s `registry`): in-crate codegen unit tests call `compile`
/// directly to inspect the emitted object, but that object is never linked into a
/// final binary and never dead-stripped, and no symbol file is staged for it.
/// There, the precondition does not apply, so the symbol set is empty — the
/// resolver emits its trivial zero-entry "always not found" form (no relocations),
/// which is exactly correct for an object no intrinsic is ever resolved through.
pub fn runtime_intrinsic_symbols_required() -> std::collections::BTreeSet<String> {
    if let Some(symbols) = runtime_intrinsic_symbols_from_env() {
        return symbols;
    }
    #[cfg(test)]
    {
        std::collections::BTreeSet::new()
    }
    #[cfg(not(test))]
    {
        panic!(
            "native backend cannot emit the per-app intrinsic resolver without the \
             linked runtime staticlib's intrinsic-symbol set. \
             `MOLT_RUNTIME_INTRINSIC_SYMBOLS` was unset or pointed at an empty/unreadable \
             file. The CLI must extract the staticlib's `molt_*` text symbols (via `nm \
             --defined-only`) and expose the path before codegen; without it the resolver \
             would emit dangling relocations against absent symbols and corrupt the binary. \
             Verify `nm`/`llvm-nm` is on PATH and the runtime staticlib built successfully."
        )
    }
}
