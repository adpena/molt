use crate::FunctionIR;
use std::collections::BTreeSet;

/// Compute the per-app callable manifest, obtaining the linked runtime
/// staticlib's callable symbol set on demand and failing closed exactly when it
/// matters.
///
/// Native and browser package resolvers must address-take only the runtime
/// callables the app can reach dynamically: intrinsic names stored as
/// `const_str` values and Python builtin functions materialized by `builtin_func`
/// ops. Both are resolved through the same app-owned callable resolver, so the
/// manifest is the single source of truth for tree-shaking that surface.
///
/// A module with no `molt_`-prefixed candidate string and no `molt_`-prefixed
/// `builtin_func` has a necessarily-empty manifest under any symbol set. That
/// keeps empty backend probes from requiring a staged staticlib symbol file.
pub fn compute_app_callable_manifest_checked(functions: &[FunctionIR]) -> BTreeSet<String> {
    let any_candidate = functions.iter().any(|f| {
        f.ops.iter().any(|op| match op.kind.as_str() {
            "const_str" | "builtin_func" => op
                .s_value
                .as_deref()
                .is_some_and(|value| value.starts_with("molt_")),
            _ => false,
        })
    });
    if !any_candidate {
        return BTreeSet::new();
    }
    let runtime_callable_symbols =
        crate::runtime_callable_symbols::runtime_callable_symbols_required();
    compute_app_callable_manifest(functions, &runtime_callable_symbols)
}

pub fn compute_app_callable_manifest(
    functions: &[FunctionIR],
    runtime_callable_symbols: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut manifest_callable_names: BTreeSet<String> = BTreeSet::new();
    for func_ir in functions {
        for op in &func_ir.ops {
            match op.kind.as_str() {
                "const_str" | "builtin_func" => {
                    if let Some(name) = op.s_value.as_deref()
                        && is_candidate_runtime_callable_name(name, runtime_callable_symbols)
                    {
                        manifest_callable_names.insert(name.to_owned());
                    }
                }
                _ => {}
            }
        }
    }
    manifest_callable_names
}

/// Decide whether a name can be safely address-taken by the app callable
/// resolver.
///
/// Membership in the linked staticlib's exact `molt_*` text-symbol set is the
/// authoritative filter. It excludes diagnostic strings that merely begin with
/// `molt_` and feature-gated runtime functions absent from the active profile,
/// so the resolver never emits dangling relocations.
fn is_candidate_runtime_callable_name(
    name: &str,
    runtime_callable_symbols: &BTreeSet<String>,
) -> bool {
    runtime_callable_symbols.contains(name)
}
