use super::runtime_roots::is_protected_runtime_entrypoint;
use crate::SimpleIR;
use std::collections::{BTreeMap, BTreeSet};

/// Dead-function elimination: remove functions that are never referenced from
/// any reachable function.  The entry function (first in the list, typically
/// `<module>`) is always retained; any function reachable from it through
/// `call_internal`, `func_new`, `func_new_closure`, `func_new_builtin`,
/// or `code_new` references is kept.
///
/// This pass runs after inlining — if a callee was fully inlined into all
/// call sites, it becomes unreachable and will be eliminated here.
/// Applies to both native and WASM backends.
pub fn eliminate_dead_functions(ir: &mut SimpleIR) {
    if std::env::var("MOLT_DISABLE_DEAD_FUNC_ELIM").is_ok() {
        return;
    }
    if ir.functions.is_empty() {
        return;
    }

    // Build the call graph: function name -> set of referenced function names.
    // Use owned Strings so that `ir.functions` is not borrowed when we call retain().
    let defined: BTreeSet<String> = ir.functions.iter().map(|f| f.name.clone()).collect();
    let mut references: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for func in &ir.functions {
        let mut refs: BTreeSet<String> = BTreeSet::new();
        for op in &func.ops {
            match op.kind.as_str() {
                "call" | "call_internal" | "func_new" | "func_new_closure" | "func_new_builtin"
                | "code_new" | "call_guarded" => {
                    if let Some(name) = op.s_value.as_ref()
                        && defined.contains(name.as_str())
                    {
                        refs.insert(name.clone());
                    }
                }
                "call_indirect" => {
                    if let Some(name) = op.s_value.as_ref()
                        && defined.contains(name.as_str())
                    {
                        refs.insert(name.clone());
                    }
                }
                // alloc_task's s_value is the poll function name directly
                // (e.g., "foo_poll"). generator_create/coro_create reference
                // a base function whose companion _poll must also be kept.
                "alloc_task" | "generator_create" | "coro_create" => {
                    if let Some(name) = op.s_value.as_ref() {
                        if defined.contains(name.as_str()) {
                            refs.insert(name.clone());
                        }
                        // generator_create/coro_create reference the base
                        // function; the backends derive "{base}_poll" at
                        // compile time, so mark both.
                        if !name.ends_with("_poll") {
                            let poll_name = format!("{name}_poll");
                            if defined.contains(poll_name.as_str()) {
                                refs.insert(poll_name);
                            }
                        }
                    }
                }
                // Ops that take a function pointer address via s_value.
                "fn_ptr_code_set" | "asyncgen_locals_register" | "gen_locals_register" => {
                    if let Some(name) = op.s_value.as_ref()
                        && defined.contains(name.as_str())
                    {
                        refs.insert(name.clone());
                    }
                }
                // Other op kinds that legitimately reference functions by name.
                "task_new" | "generator_send" | "spawn" | "call_func" | "call_method"
                | "import_from" | "import_name" | "class_def" | "decorator" | "super_call"
                | "yield_from" | "await" => {
                    if let Some(name) = op.s_value.as_ref()
                        && defined.contains(name.as_str())
                    {
                        refs.insert(name.clone());
                    }
                }
                _ => {}
            }
        }
        references.insert(func.name.clone(), refs);
    }

    // BFS from entry roots to find all reachable functions.
    // Roots: (1) the first function (entry), (2) well-known linker/runtime
    // entry points, (3) any function whose name matches a keep-pattern.
    let mut reachable: BTreeSet<String> = BTreeSet::new();
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::new();

    let seed =
        |name: String, r: &mut BTreeSet<String>, q: &mut std::collections::VecDeque<String>| {
            if r.insert(name.clone()) {
                q.push_back(name);
            }
        };

    // (1) First function is always the module entry.
    seed(ir.functions[0].name.clone(), &mut reachable, &mut queue);

    // (2) + (3) Scan all functions for keep-patterns.
    //
    // molt_init_* functions are NOT blanket-kept.  They are referenced by
    // static CALL ops in the IR (emitted by the frontend's _emit_module_load)
    // so the BFS discovers them naturally.
    //
    // molt_isolate_* functions MUST be kept with their full bodies because
    // the runtime references them as extern "C" symbols for dynamic imports
    // and isolate startup. Stubbing them based on local reachability breaks
    // Python-level import paths (`__import__`, importlib helpers, intrinsic
    // module loads) that route through the runtime rather than direct IR edges.
    // Binary size should be controlled by the module graph itself, not by
    // mutating the semantics of runtime entrypoints during DFE.
    for func in &ir.functions {
        if is_protected_runtime_entrypoint(&func.name) {
            seed(func.name.clone(), &mut reachable, &mut queue);
        }
    }

    while let Some(current) = queue.pop_front() {
        if let Some(refs) = references.get(&current) {
            for target in refs {
                if reachable.insert(target.clone()) {
                    queue.push_back(target.clone());
                }
            }
        }
    }

    let original_count = ir.functions.len();
    ir.functions.retain(|f| reachable.contains(&f.name));
    let eliminated = original_count - ir.functions.len();

    if eliminated > 0 && std::env::var("MOLT_DEBUG_DEAD_FUNC_ELIM").is_ok() {
        eprintln!(
            "dead-func-elim: removed {eliminated} of {original_count} functions ({} retained)",
            ir.functions.len()
        );
    }
}
