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
    /// Exact compiled Python bodies admitted by compiler-owned GPU metadata.
    /// These are application functions, never runtime/staticlib imports.
    pub compiled_body_symbols: BTreeSet<String>,
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

/// Compiler metadata creates executable identity edges without allocating a
/// second Python callable. Only the internal publisher consumes this JSON;
/// merely spelling one of these symbols in guest data creates no edge.
pub(crate) fn visit_gpu_descriptor_body_symbols(
    function: &FunctionIR,
    mut visit: impl FnMut(&str),
) {
    if !function.ops.iter().any(|op| {
        op.kind == molt_ir::OpKind::Call
            && op.s_value.as_deref() == Some("molt_gpu_kernel_descriptor_set")
    }) {
        return;
    }
    let constants: BTreeMap<&str, &str> = function
        .ops
        .iter()
        .filter_map(|op| {
            (op.kind == molt_ir::OpKind::ConstStr).then_some(())?;
            Some((op.out.as_deref()?, op.s_value.as_deref()?))
        })
        .collect();
    for op in &function.ops {
        if op.kind != molt_ir::OpKind::Call
            || op.s_value.as_deref() != Some("molt_gpu_kernel_descriptor_set")
        {
            continue;
        }
        let payload = op
            .args
            .as_ref()
            .and_then(|args| args.get(1))
            .and_then(|name| constants.get(name.as_str()))
            .unwrap_or_else(|| panic!("GPU metadata publisher lost its constant descriptor"));
        let descriptor: serde_json::Value = serde_json::from_str(payload)
            .unwrap_or_else(|error| panic!("invalid compiler GPU descriptor: {error}"));
        if let Some(bodies) = descriptor
            .get("python_bodies")
            .and_then(serde_json::Value::as_object)
        {
            for body in bodies.values() {
                let symbol = body
                    .get("symbol")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_else(|| panic!("GPU body identity lacks its executable symbol"));
                visit(symbol);
            }
        }
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
        visit_gpu_descriptor_body_symbols(function, |symbol| {
            assert!(
                defined_functions.contains(symbol),
                "GPU body identity names an absent application function: {symbol}"
            );
            requirements.compiled_body_symbols.insert(symbol.to_owned());
        });
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
                if op.kind == molt_ir::OpKind::ConstStr
                    && definitions.get(op.out.as_deref()?) == Some(&1)
                {
                    Some((op.out.as_deref()?, op.s_value.as_deref()?))
                } else {
                    None
                }
            })
            .collect();
        for op in &function.ops {
            let is_module_publication = op.kind == molt_ir::OpKind::ModuleCacheSet
                || (op.kind == molt_ir::OpKind::Call
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
                // The public builtin namespace aliases foreign providers;
                // a provider's own publication exposes only its declarations.
                // These remain optional profile candidates on every backend.
                requirements.builtin_namespace_trampolines.extend(
                    PYTHON_BUILTIN_CALLABLES
                        .iter()
                        .filter(|spec| {
                            name.is_none()
                                || name == Some("builtins")
                                || name == Some(spec.python_module)
                        })
                        .map(|spec| (spec.runtime_name.to_owned(), spec.arity)),
                );
            }
            if matches!(
                op.kind,
                molt_ir::OpKind::ConstStr | molt_ir::OpKind::BuiltinFunc
            ) && let Some(name) = op
                .s_value
                .as_deref()
                // The runtime's explicit `_molt_` alias resolves the same
                // canonical provider; resolver tables contain primary names.
                .map(|name| name.strip_prefix('_').unwrap_or(name))
                .filter(|name| name.starts_with("molt_"))
            {
                requirements.intrinsic_names.insert(name.to_owned());
            }
            if let Some(name) = &op.runtime_symbol {
                requirements.intrinsic_names.insert(name.clone());
            }
            let is_global_lookup = op.kind == molt_ir::OpKind::ModuleGetGlobal
                || (op.kind == molt_ir::OpKind::Call
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

/// What one app-resolver entry names. A runtime callable is a staticlib
/// symbol with an execution ABI; a compiled body is an application function
/// this program defines. The two are different identities and are never
/// told apart by their spelling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AppCallableKind {
    /// A runtime entry, by its typed identity (HF-821 B1).
    RuntimeCallable(molt_ir::runtime_fn_generated::RuntimeFn),
    CompiledBody,
}

/// The per-app callable manifest: every name the app resolver answers, with
/// its kind. Both native resolvers (Cranelift and LLVM) and the batch worker's
/// job transport read this one record.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AppCallableManifest {
    pub runtime_callables: BTreeSet<String>,
    pub compiled_bodies: BTreeSet<String>,
}

impl AppCallableManifest {
    pub fn is_empty(&self) -> bool {
        self.runtime_callables.is_empty() && self.compiled_bodies.is_empty()
    }

    pub fn len(&self) -> usize {
        self.runtime_callables.len() + self.compiled_bodies.len()
    }

    /// Every entry with its kind, in unsigned-byte name order: the order the
    /// resolver's binary search requires. A name of both kinds is a compiler
    /// defect and panics.
    pub fn entries(&self) -> BTreeMap<&str, AppCallableKind> {
        let mut entries = BTreeMap::new();
        for name in &self.runtime_callables {
            // The manifest's names come from the runtime callable surface, a
            // subset of the generated RuntimeFn table: decoded here, once.
            let function = molt_ir::runtime_fn_generated::RuntimeFn::from_symbol(name)
                .unwrap_or_else(|| panic!("app runtime callable `{name}` has no RuntimeFn"));
            entries.insert(name.as_str(), AppCallableKind::RuntimeCallable(function));
        }
        for name in &self.compiled_bodies {
            assert!(
                entries
                    .insert(name.as_str(), AppCallableKind::CompiledBody)
                    .is_none(),
                "app callable `{name}` is both a runtime callable and a compiled body"
            );
        }
        entries
    }

    /// Every name, of either kind.
    pub fn names(&self) -> impl Iterator<Item = &String> {
        self.runtime_callables.iter().chain(&self.compiled_bodies)
    }
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
pub fn compute_app_callable_manifest_checked(functions: &[FunctionIR]) -> AppCallableManifest {
    let mut requirements = collect_app_callable_requirements(functions);
    let compiled_bodies = std::mem::take(&mut requirements.compiled_body_symbols);
    let mut runtime_callables = requirements.into_names();
    if !runtime_callables.is_empty() {
        let binding = crate::runtime_callable_symbols::RuntimeCallableSymbols::capture();
        let symbols = binding.required_symbols();
        runtime_callables.retain(|name| symbols.contains(name));
    }
    AppCallableManifest {
        runtime_callables,
        compiled_bodies,
    }
}

pub fn compute_app_callable_manifest(
    functions: &[FunctionIR],
    runtime_callable_symbols: &BTreeSet<String>,
) -> AppCallableManifest {
    let mut requirements = collect_app_callable_requirements(functions);
    let compiled_bodies = std::mem::take(&mut requirements.compiled_body_symbols);
    let mut runtime_callables = requirements.into_names();
    runtime_callables.retain(|name| runtime_callable_symbols.contains(name));
    AppCallableManifest {
        runtime_callables,
        compiled_bodies,
    }
}
