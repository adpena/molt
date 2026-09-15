use crate::FunctionIR;
use molt_ir::python_builtin_callables_generated::{
    PYTHON_BUILTIN_CALLABLES, python_builtin_callable,
};
use molt_ir::tir::simple_def_use::visit_simple_ir_defined_names;
use std::collections::{BTreeMap, BTreeSet};

/// Backend-independent, possible runtime-callable reachability. These edges
/// retain targets; they never specialize a mutable Python binding.
#[derive(Default)]
pub struct AppCallableRequirements {
    pub builtin_trampolines: BTreeMap<String, usize>,
    /// Namespace publication exposes only providers admitted by the target;
    /// unlike actual global lookups, these are not mandatory profile roots.
    pub builtin_namespace_trampolines: BTreeMap<String, usize>,
    pub intrinsic_names: BTreeSet<String>,
}

fn builtin_callable_family() -> impl Iterator<Item = (String, usize)> {
    PYTHON_BUILTIN_CALLABLES
        .iter()
        .map(|spec| (spec.runtime_name.to_owned(), spec.arity))
}

impl AppCallableRequirements {
    fn record_builtin_lookup(&mut self, name: Option<&str>) {
        if let Some(name) = name {
            if let Some(spec) = python_builtin_callable(name) {
                self.builtin_trampolines
                    .insert(spec.runtime_name.to_owned(), spec.arity);
            }
        } else {
            self.builtin_trampolines.extend(builtin_callable_family());
        }
    }

    fn into_names(self) -> BTreeSet<String> {
        let mut names = self.intrinsic_names;
        names.extend(self.builtin_trampolines.into_keys());
        names.extend(self.builtin_namespace_trampolines.into_keys());
        names
    }
}

/// One collector for native app resolvers and WASM imports/table/resolvers.
/// Literal intrinsic names can flow through aliases, wrapper arguments, or
/// object fields (e.g. sys._LazyIntrinsic); a direct-call-pattern scan is unsound.
/// Each backend admits candidates against its linked runtime/ABI authority.
pub fn collect_app_callable_requirements(functions: &[FunctionIR]) -> AppCallableRequirements {
    let defined_functions: BTreeSet<&str> = functions.iter().map(|f| f.name.as_str()).collect();
    let mut requirements = AppCallableRequirements::default();
    for function in functions {
        // SimpleIR is mutable transport, not SSA. A whole-function constant
        // needs exactly one definition, including parameters and store targets.
        let mut definitions = BTreeMap::<&str, usize>::new();
        for name in &function.params {
            *definitions.entry(name).or_default() += 1;
        }
        for op in &function.ops {
            visit_simple_ir_defined_names(op, |name| {
                *definitions.entry(name).or_default() += 1;
            });
        }
        let const_strings: BTreeMap<&str, &str> = function
            .ops
            .iter()
            .filter_map(|op| {
                if op.kind == "const_str" && definitions.get(op.out.as_deref()?) == Some(&1) {
                    Some((op.out.as_deref()?, op.s_value.as_deref()?))
                } else {
                    None
                }
            })
            .collect();
        for op in &function.ops {
            let is_module_publication = op.kind == "module_cache_set"
                || (op.kind == "call"
                    && op.s_value.as_deref().is_some_and(|name| {
                        matches!(name, "molt_module_cache_set" | "module_cache_set")
                            && !defined_functions.contains(name)
                    }));
            if is_module_publication {
                let name = op
                    .args
                    .as_deref()
                    .and_then(|args| args.first())
                    .and_then(|name| const_strings.get(name.as_str()).copied());
                if name.is_none() || name == Some("builtins") {
                    requirements
                        .builtin_namespace_trampolines
                        .extend(builtin_callable_family());
                }
            }
            if matches!(op.kind.as_str(), "const_str" | "builtin_func") {
                if let Some(name) = op
                    .s_value
                    .as_deref()
                    // The runtime's explicit `_molt_` alias resolves the same
                    // canonical provider; resolver tables contain primary names.
                    .map(|name| name.strip_prefix('_').unwrap_or(name))
                    .filter(|name| name.starts_with("molt_"))
                {
                    requirements.intrinsic_names.insert(name.to_owned());
                }
            }
            if let Some(name) = &op.runtime_symbol {
                requirements.intrinsic_names.insert(name.clone());
            }
            let is_global_lookup = op.kind == "module_get_global"
                || (op.kind == "call"
                    && op.s_value.as_deref().is_some_and(|name| {
                        matches!(name, "molt_module_get_global" | "module_get_global")
                            && !defined_functions.contains(name)
                    }));
            if is_global_lookup {
                let name = op
                    .args
                    .as_deref()
                    .and_then(|args| args.get(1))
                    .and_then(|name| const_strings.get(name.as_str()).copied());
                requirements.record_builtin_lookup(name);
            }
        }
    }
    requirements
}

/// Compute the per-app callable manifest, obtaining the linked runtime
/// staticlib's callable symbol set on demand and failing closed exactly when it
/// matters.
///
/// Native and browser package resolvers consume the same possible callable
/// requirements as WASM's table/import planner. Exact linked-staticlib symbol
/// membership excludes diagnostics and intrinsics absent from this profile.
///
/// Empty requirements keep backend probes independent of a staged symbol file;
/// nonempty requirements are collected once and then admitted once.
pub fn compute_app_callable_manifest_checked(functions: &[FunctionIR]) -> BTreeSet<String> {
    let mut names = collect_app_callable_requirements(functions).into_names();
    if !names.is_empty() {
        let symbols = crate::runtime_callable_symbols::runtime_callable_symbols_required();
        names.retain(|name| symbols.contains(name));
    }
    names
}

pub fn compute_app_callable_manifest(
    functions: &[FunctionIR],
    runtime_callable_symbols: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut names = collect_app_callable_requirements(functions).into_names();
    names.retain(|name| runtime_callable_symbols.contains(name));
    names
}
