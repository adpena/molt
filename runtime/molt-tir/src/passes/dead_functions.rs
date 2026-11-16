use super::runtime_roots::is_protected_runtime_entrypoint;
use crate::SimpleIR;
use molt_ir::backend_environment::{self as backend_env, BackendEnv};
use molt_ir::{FunctionId, FunctionSet, FunctionTable};

use std::collections::{BTreeSet, HashMap, VecDeque};

/// Exact executable symbol references carried by a function: the linker
/// symbol of each typed callee (resolved through the function's own document
/// `table`), and the body symbols a compiler-owned GPU descriptor embeds.
/// Callers decide whether a target is defined, external, or forbidden across
/// their boundary; this collector does not infer companion symbols or treat
/// arbitrary string literals as references.
pub fn visit_function_reference_targets(
    function: &crate::FunctionIR,
    table: &FunctionTable,
    mut visit: impl FnMut(&str),
) {
    super::app_callable_manifest::visit_gpu_descriptor_body_symbols(function, &mut visit);
    for op in &function.ops {
        if let Some(callee) = op.callee {
            visit(table.symbol(callee));
        }
    }
}

/// Dead-function elimination: remove functions that are never referenced from
/// any reachable function. The entry function (first in the list, typically
/// `<module>`) is always retained; any function reachable from it through a
/// typed callee (HF-847 slice 1) is kept. Ids are compacted through
/// `retain_functions`, so every surviving callee still names its row.
///
/// This pass runs after inlining — if a callee was fully inlined into all
/// call sites, it becomes unreachable and will be eliminated here.
/// Applies to both native and WASM backends.
pub fn eliminate_dead_functions(ir: &mut SimpleIR) {
    eliminate_dead_functions_with_roots(ir, &BTreeSet::new());
}

/// Dead-function elimination with additional named roots.
///
/// The import-bedrock registry lane (design doc 69) reaches module init
/// bodies ONLY through `MODULE_INIT_TABLE` function-address relocations in
/// the registry blob — there are no `call` edges to them in the IR — so the
/// native application-object pipeline seeds `module_registry.init_symbols`
/// here (linker symbols, the registry's own identity). Every native DFE
/// invocation on IR that carries a module registry MUST pass those roots or
/// the registry blob relocations dangle at link.
pub fn eliminate_dead_functions_with_roots(ir: &mut SimpleIR, extra_roots: &BTreeSet<String>) {
    let reachable = reachable_functions(&ir.functions, extra_roots);
    let original_count = ir.functions.len();
    let mut index = 0usize;
    molt_ir::function_table::retain_functions(&mut ir.functions, |_| {
        index += 1;
        reachable.contains(FunctionId::from_index(index - 1))
    });
    let eliminated = original_count - ir.functions.len();

    if eliminated > 0 && backend_env::var(BackendEnv::MOLT_DEBUG_DEAD_FUNC_ELIM).is_ok() {
        eprintln!(
            "dead-func-elim: removed {eliminated} of {original_count} functions ({} retained)",
            ir.functions.len()
        );
    }
}

/// One reachability decision for both source-only collections and native
/// collections that retain final body/fact custody. Callers retain whole rows
/// by id, then compact through `retain_functions`.
pub fn reachable_functions(
    functions: &[crate::FunctionIR],
    extra_roots: &BTreeSet<String>,
) -> FunctionSet {
    let mut reachable = FunctionSet::with_capacity(functions.len());
    if backend_env::var(BackendEnv::MOLT_DISABLE_DEAD_FUNC_ELIM).is_ok() {
        for index in 0..functions.len() {
            reachable.insert(FunctionId::from_index(index));
        }
        return reachable;
    }
    if functions.is_empty() {
        return reachable;
    }
    // GPU descriptors embed body symbols in a constant string; only they need
    // the symbol index.
    let by_symbol: HashMap<&str, FunctionId> = functions
        .iter()
        .enumerate()
        .map(|(index, function)| (&*function.name, FunctionId::from_index(index)))
        .collect();

    // Roots: (1) the first function (entry), (2) protected runtime entry
    // points and registry roots, by their linker symbols.
    let mut queue: VecDeque<FunctionId> = VecDeque::new();
    let seed = |id: FunctionId, reachable: &mut FunctionSet, queue: &mut VecDeque<_>| {
        if reachable.insert(id) {
            queue.push_back(id);
        }
    };
    seed(FunctionId(0), &mut reachable, &mut queue);
    // Module init bodies are NOT blanket-kept: the frontend's module loads
    // call them, so the BFS discovers them. molt_isolate_* functions are kept
    // whole: the runtime calls them as extern "C" symbols for dynamic imports
    // and isolate startup, which no IR edge shows.
    for (index, function) in functions.iter().enumerate() {
        if is_protected_runtime_entrypoint(&function.name) || extra_roots.contains(&function.name) {
            seed(FunctionId::from_index(index), &mut reachable, &mut queue);
        }
    }
    while let Some(current) = queue.pop_front() {
        let function = &functions[current.index()];
        let mut targets: Vec<FunctionId> = function.ops.iter().filter_map(|op| op.callee).collect();
        super::app_callable_manifest::visit_gpu_descriptor_body_symbols(function, |symbol| {
            if let Some(&id) = by_symbol.get(symbol) {
                targets.push(id);
            }
        });
        for target in targets {
            if reachable.insert(target) {
                queue.push_back(target);
            }
        }
    }
    reachable
}
