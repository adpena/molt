use crate::FunctionIR;
use std::collections::BTreeSet;

/// Compute the set of intrinsic names the app resolves through the name-based
/// runtime lookup path (`molt_require_intrinsic_runtime` /
/// `molt_load_intrinsic_runtime`).  This mirrors the WASM manifest scan in
/// `wasm.rs` (the `manifest_intrinsic_names` construction): for each function,
/// a `const_str` op's output is the candidate intrinsic name, a `builtin_func`
/// op producing the runtime-lookup helper marks its output as a lookup var, and
/// a `call_func` whose first arg is a lookup var and whose second arg is a
/// const-string output names the intrinsic the app reaches by name.
///
/// The native backend emits a per-app Cranelift resolver covering exactly this
/// set, so `resolve_symbol`/`resolve_core_symbol` (which address-take every
/// intrinsic) become native-unreachable and are dead-stripped.
///
/// This MUST be called over ALL functions including externs (before the
/// `is_extern` filter), so the `stdlib_shared.o` partition's intrinsic uses are
/// covered.
/// Compute the per-app intrinsic manifest, obtaining the linked runtime
/// staticlib's symbol set on demand — fail-closed exactly when it matters.
///
/// A module with NO `molt_`-prefixed `const_str` anywhere (the CLI's empty
/// post-build feature-probe module, or an app that reaches every intrinsic by
/// direct call) has a necessarily-empty manifest under ANY symbol set: every
/// manifest entry is a `const_str` value that is a member of the symbol set, and
/// all runtime intrinsics are `molt_`-prefixed (the CLI extractor keeps exactly
/// the staticlib's `molt_*` text symbols). The resolver then emits its trivial
/// zero-entry "always not found" form with no relocations, so the staticlib
/// symbol set is not a precondition there. Requiring it unconditionally made the
/// CLI's post-build backend feature probe — which runs the backend on an empty
/// module with no symbol file staged — panic in
/// [`runtime_intrinsic_symbols_required`](crate::intrinsic_symbols::runtime_intrinsic_symbols_required),
/// wedging every backend rebuild behind a "feature mismatch; cleaning and
/// rebuilding" loop. For any module that COULD name an intrinsic (some
/// `molt_`-prefixed `const_str` exists), the symbol set remains a hard
/// precondition and absence still fails the build closed — the
/// dangling-relocation corruption class this guards is unchanged.
///
/// This is the single manifest entry point for every resolver-emitting caller
/// (native `SimpleBackend::compile`, the orchestrator's split/batch paths); the
/// two-argument [`compute_intrinsic_manifest`] is the set-supplied core it and
/// the unit tests share.
pub fn compute_intrinsic_manifest_checked(functions: &[FunctionIR]) -> BTreeSet<String> {
    let any_candidate = functions.iter().any(|f| {
        f.ops.iter().any(|op| {
            op.kind == "const_str"
                && op
                    .s_value
                    .as_deref()
                    .is_some_and(|v| v.starts_with("molt_"))
        })
    });
    if !any_candidate {
        return BTreeSet::new();
    }
    let runtime_intrinsic_symbols = crate::intrinsic_symbols::runtime_intrinsic_symbols_required();
    compute_intrinsic_manifest(functions, &runtime_intrinsic_symbols)
}

pub fn compute_intrinsic_manifest(
    functions: &[FunctionIR],
    runtime_intrinsic_symbols: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut manifest_intrinsic_names: BTreeSet<String> = BTreeSet::new();
    // Every `const_str` whose value names a runtime intrinsic is a candidate the
    // app may resolve by name at runtime. The intrinsic name reaches
    // `require_intrinsic` / `load_intrinsic` either directly (`require_intrinsic(
    // "molt_foo")`) or — crucially — through a stdlib wrapper such as
    // `_require_callable_intrinsic("molt_gc_collect")` or `gc.py`'s
    // `_require_intrinsic(name)` where `name` flows from a constant. The narrow
    // `call_func(runtime_lookup_var, const_str)` shape only catches the direct,
    // single-call case and misses every wrapper-indirected name, so the resolver
    // would return 0 for them and the runtime would raise "intrinsic unavailable"
    // (and, because `resolve_core_symbol` is dead-stripped on native, the symbol
    // is not even present). Capturing the const-string names structurally — and
    // validating them against the symbols the linked runtime staticlib actually
    // defines — is the complete, robust manifest.
    // Any `const_str` whose value is a real runtime intrinsic symbol is a name
    // the app may resolve dynamically. The name reaches `require_intrinsic` /
    // `load_intrinsic` through arbitrary data flow: directly
    // (`require_intrinsic("molt_foo")`), through a wrapper call
    // (`_require_callable_intrinsic("molt_gc_collect")`), or — crucially — stored
    // in an object field and read back later (sys.py's
    // `_LazyIntrinsic("molt_sys_version_info", default)` stashes the name in
    // `self._name` and calls `_require_intrinsic(self._name)` on first use). A
    // call-argument-only scan misses the object-field case, which silently
    // degrades to the wrapper's fallback value (e.g. `sys.version_info` reverting
    // to the 3.12 default under a 3.13 target) rather than crashing — exactly the
    // class of bug a too-narrow manifest produces. Capturing every const_str
    // whose value is a real intrinsic symbol is the only data-flow-complete
    // manifest. The `is_candidate_intrinsic_name` filter (exact membership in the
    // linked staticlib's intrinsic symbol set) keeps this precise: it excludes
    // free-text strings that merely begin with `molt_` and intrinsics feature-gated
    // out of the active stdlib profile, so the resolver never takes the address of a
    // symbol the linker cannot resolve, and `-dead_strip` still removes every
    // intrinsic whose name appears nowhere as a string constant. The symbol set is
    // required (no heuristic fallback): an unknown set fails the build closed at the
    // caller rather than guessing and re-creating the dangling-relocation corruption.
    // The runtime resolves intrinsics through the per-app resolver by manifest
    // symbol. Intrinsic names are required to match linker symbols, so the
    // manifest scan keys directly on the captured const string and fails closed
    // when a name has no runtime symbol.
    for func_ir in functions {
        for op in &func_ir.ops {
            if op.kind == "const_str"
                && let Some(val) = op.s_value.as_deref()
                && is_candidate_intrinsic_name(val, runtime_intrinsic_symbols)
            {
                manifest_intrinsic_names.insert(val.to_owned());
            }
        }
    }
    manifest_intrinsic_names
}

/// Decide whether a `const_str` value names a runtime intrinsic the app resolver
/// may safely take the address of.
///
/// Membership in the set of intrinsic symbols the linked runtime staticlib
/// *defines* (extracted by the CLI for the active stdlib profile and threaded in)
/// is the authoritative, exact filter: it excludes diagnostic strings that merely
/// begin with `molt_` (e.g. `"molt_sys_platform intrinsic unavailable"`) AND
/// intrinsics that are feature-gated out of the active stdlib profile (e.g. crypto
/// on the micro profile). Taking the address of an absent symbol via a pointer
/// relocation would leave an unresolved relocation the linker cannot satisfy — the
/// precise cause of the link failure / Mach-O header corruption this resolver
/// design exists to prevent.
///
/// There is deliberately NO heuristic fallback: a `molt_`-prefixed identifier that
/// passes a structural shape check can still be absent from the active profile's
/// staticlib, so guessing re-creates the dangling-relocation corruption class. The
/// exact symbol set is therefore a hard precondition — native callers that feed
/// the resolver obtain it via `runtime_intrinsic_symbols_required`, which fails the
/// build closed (with an actionable diagnostic) when it is unavailable rather than
/// emitting a corrupt binary.
fn is_candidate_intrinsic_name(name: &str, runtime_intrinsic_symbols: &BTreeSet<String>) -> bool {
    runtime_intrinsic_symbols.contains(name)
}

// ---------------------------------------------------------------------------
