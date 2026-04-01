//! Monomorphization pass: generate type-specialized copies of generic functions.
//!
//! When a function is called with concrete argument types, this pass creates
//! a specialized copy where `DynBox` parameters are replaced with the concrete
//! types. Type refinement is then run on the copy to propagate the concrete
//! types through all operations.
//!
//! # Example
//! ```text
//! def add(x, y):           # generic: DynBox, DynBox → DynBox
//!     return x + y
//!
//! add(1, 2)                # → add__I64_I64(x: I64, y: I64) → I64
//! add(1.0, 2.0)            # → add__F64_F64(x: F64, y: F64) → F64
//! ```
//!
//! # Depth limit
//! To prevent exponential blowup from recursive specialization, a depth limit
//! of 4 is enforced. Beyond that, calls are left as generic.

use std::collections::HashMap;

use crate::tir::blocks::BlockId;
use crate::tir::function::{TirFunction, TirModule};
use crate::tir::ops::{AttrValue, OpCode};
use crate::tir::type_refine;
use crate::tir::types::TirType;
use crate::tir::values::ValueId;

/// Maximum specialization depth to prevent exponential blowup.
const MAX_DEPTH: usize = 4;

/// Cache key: (original_function_name, concrete_arg_types) → specialized_name.
type SpecCache = HashMap<(String, Vec<TirType>), String>;

/// Mangle a type into a short string suffix for use in specialized function names.
fn mangle_type(ty: &TirType) -> String {
    match ty {
        TirType::I64 => "I64".to_string(),
        TirType::F64 => "F64".to_string(),
        TirType::Bool => "Bool".to_string(),
        TirType::None => "None".to_string(),
        TirType::Str => "Str".to_string(),
        TirType::Bytes => "Bytes".to_string(),
        TirType::BigInt => "BigInt".to_string(),
        TirType::DynBox => "Dyn".to_string(),
        TirType::Never => "Never".to_string(),
        TirType::List(inner) => format!("List{}", mangle_type(inner)),
        TirType::Set(inner) => format!("Set{}", mangle_type(inner)),
        TirType::Dict(k, v) => format!("Dict{}x{}", mangle_type(k), mangle_type(v)),
        TirType::Tuple(elems) => {
            let inner: Vec<String> = elems.iter().map(mangle_type).collect();
            format!("Tuple{}", inner.join("x"))
        }
        TirType::Box(inner) => format!("Box{}", mangle_type(inner)),
        TirType::Ptr(inner) => format!("Ptr{}", mangle_type(inner)),
        TirType::Func(_) => "Func".to_string(),
        TirType::Union(members) => {
            let inner: Vec<String> = members.iter().map(mangle_type).collect();
            format!("Union{}", inner.join("x"))
        }
    }
}

/// Returns `true` if all types are concrete (no `DynBox` anywhere at the top level).
fn all_concrete(types: &[TirType]) -> bool {
    types.iter().all(|ty| !matches!(ty, TirType::DynBox))
}

/// Build a type environment (ValueId → TirType) for a function by scanning
/// all block arguments and op results.
fn build_type_env(func: &TirFunction) -> HashMap<ValueId, TirType> {
    let mut env: HashMap<ValueId, TirType> = HashMap::new();
    for block in func.blocks.values() {
        for arg in &block.args {
            env.insert(arg.id, arg.ty.clone());
        }
        for op in &block.ops {
            for &result_id in &op.results {
                env.entry(result_id).or_insert(TirType::DynBox);
            }
        }
    }
    env
}

/// Extract the statically-known callee name from a Call op's attrs.
fn callee_name(attrs: &HashMap<String, AttrValue>) -> Option<String> {
    match attrs.get("callee").or_else(|| attrs.get("s_value")) {
        Some(AttrValue::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Create a deep clone of a `TirFunction` with a new name.
///
/// All blocks, ops, and values are cloned verbatim; only the function name
/// changes at this stage. Parameter types are replaced separately.
fn clone_function(func: &TirFunction, new_name: String) -> TirFunction {
    TirFunction {
        name: new_name,
        param_names: func.param_names.clone(),
        param_types: func.param_types.clone(),
        return_type: func.return_type.clone(),
        blocks: func.blocks.clone(),
        entry_block: func.entry_block,
        next_value: func.next_value,
        next_block: func.next_block,
        attrs: func.attrs.clone(),
        has_exception_handling: func.has_exception_handling,
        label_id_map: func.label_id_map.clone(),
        loop_roles: func.loop_roles.clone(),
        loop_pairs: func.loop_pairs.clone(),
        loop_break_kinds: func.loop_break_kinds.clone(),
        loop_break_blocks: func.loop_break_blocks.clone(),
    }
}

/// Specialize a clone of `callee` for the given concrete argument types.
///
/// 1. Clones the function under a mangled name.
/// 2. Replaces entry block argument types with the concrete types.
/// 3. Updates `param_types` to match.
/// 4. Runs `type_refine::refine_types` to propagate concrete types through
///    all operations.
fn specialize(callee: &TirFunction, concrete_args: &[TirType]) -> TirFunction {
    let suffix: String = concrete_args
        .iter()
        .map(mangle_type)
        .collect::<Vec<_>>()
        .join("_");
    let specialized_name = format!("{}__{}", callee.name, suffix);

    let mut copy = clone_function(callee, specialized_name);

    // Replace entry block argument types with the concrete types.
    let entry_id = copy.entry_block;
    if let Some(entry_block) = copy.blocks.get_mut(&entry_id) {
        for (arg, concrete_ty) in entry_block.args.iter_mut().zip(concrete_args.iter()) {
            // Only replace DynBox params; leave already-concrete types alone.
            if matches!(arg.ty, TirType::DynBox) {
                arg.ty = concrete_ty.clone();
            }
        }
    }

    // Keep `param_types` in sync.
    copy.param_types = copy.blocks[&copy.entry_block]
        .args
        .iter()
        .map(|a| a.ty.clone())
        .collect();

    // Propagate types through the rest of the function.
    type_refine::refine_types(&mut copy);

    copy
}

/// Scan all ops in a function and collect (callee_name, arg_types, call_site_location)
/// for every `Call` op whose arguments are all concrete.
///
/// Returns: Vec<(callee_name, arg_types, block_id, op_index)>
fn collect_call_sites(
    func: &TirFunction,
    env: &HashMap<ValueId, TirType>,
) -> Vec<(String, Vec<TirType>, BlockId, usize)> {
    let mut sites = Vec::new();

    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    for bid in block_ids {
        let block = &func.blocks[&bid];
        for (op_idx, op) in block.ops.iter().enumerate() {
            if op.opcode != OpCode::Call {
                continue;
            }
            let Some(callee) = callee_name(&op.attrs) else {
                continue;
            };
            // Resolve argument types from the type env.
            let arg_types: Vec<TirType> = op
                .operands
                .iter()
                .map(|id| env.get(id).cloned().unwrap_or(TirType::DynBox))
                .collect();

            // Only specialize when ALL args are concrete.
            if all_concrete(&arg_types) && !arg_types.is_empty() {
                sites.push((callee, arg_types, bid, op_idx));
            }
        }
    }

    sites
}

/// Core monomorphization driver.
///
/// Performs up to `MAX_DEPTH` rounds of specialization. Each round:
/// 1. Scans all existing functions for call sites with concrete arg types.
/// 2. For each new (callee, arg_types) pair not in the cache:
///    a. Clones and specializes the callee.
///    b. Adds the specialization to the module.
///    c. Records the mapping in the cache.
/// 3. Rewrites all call sites to target the specialized name.
///
/// Returns the total number of specializations created.
pub fn monomorphize(module: &mut TirModule) -> usize {
    let mut cache: SpecCache = HashMap::new();
    let mut total_specs = 0;

    for _depth in 0..MAX_DEPTH {
        // Collect all pending specializations this round.
        // Tuple: (caller_func_idx, block_id, op_idx, callee_name, specialized_name, arg_types)
        let mut pending: Vec<(usize, BlockId, usize, String, String, Vec<TirType>)> = Vec::new();

        // Build the function name → index map for fast lookups.
        let func_index: HashMap<String, usize> = module
            .functions
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.clone(), i))
            .collect();

        // Scan every function for specializable call sites.
        for caller_idx in 0..module.functions.len() {
            let env = build_type_env(&module.functions[caller_idx]);
            let sites = collect_call_sites(&module.functions[caller_idx], &env);

            for (callee_name, arg_types, bid, op_idx) in sites {
                // Skip if callee is not defined in this module.
                if !func_index.contains_key(&callee_name) {
                    continue;
                }

                let cache_key = (callee_name.clone(), arg_types.clone());
                let spec_name = if let Some(existing) = cache.get(&cache_key) {
                    existing.clone()
                } else {
                    // Compute the mangled name without mutating yet.
                    let suffix: String = arg_types
                        .iter()
                        .map(mangle_type)
                        .collect::<Vec<_>>()
                        .join("_");
                    format!("{}__{}", callee_name, suffix)
                };

                // Skip if already resolved to itself (no DynBox params to specialize).
                if spec_name == callee_name {
                    continue;
                }

                pending.push((caller_idx, bid, op_idx, callee_name, spec_name, arg_types));
            }
        }

        if pending.is_empty() {
            break;
        }

        // Process pending specializations — create new functions, rewrite call sites.
        let mut new_functions: Vec<TirFunction> = Vec::new();

        for (caller_idx, bid, op_idx, callee_name, spec_name, arg_types) in &pending {
            let cache_key = (callee_name.clone(), arg_types.clone());

            // Only create a new specialization if not already done this pass.
            if !cache.contains_key(&cache_key) {
                // Find the callee in the module's current functions + new functions.
                let callee_opt = module
                    .functions
                    .iter()
                    .find(|f| &f.name == callee_name)
                    .or_else(|| new_functions.iter().find(|f| &f.name == callee_name));

                if let Some(callee) = callee_opt {
                    // Only specialize if at least one param is DynBox.
                    let has_dynbox = callee
                        .param_types
                        .iter()
                        .any(|ty| matches!(ty, TirType::DynBox));

                    if has_dynbox {
                        let specialized = specialize(callee, arg_types);
                        cache.insert(cache_key.clone(), specialized.name.clone());
                        new_functions.push(specialized);
                        total_specs += 1;
                    } else {
                        // No DynBox params — function is already concrete, no specialization needed.
                        cache.insert(cache_key.clone(), callee_name.clone());
                    }
                }
            }

            // Rewrite the call site to use the specialized name.
            let actual_spec_name = cache
                .get(&cache_key)
                .cloned()
                .unwrap_or_else(|| spec_name.clone());
            if &actual_spec_name != callee_name
                && let Some(block) = module.functions[*caller_idx].blocks.get_mut(bid)
            {
                let op = &mut block.ops[*op_idx];
                // Update "callee" attr (preferred) or "s_value".
                if op.attrs.contains_key("callee") {
                    op.attrs
                        .insert("callee".to_string(), AttrValue::Str(actual_spec_name));
                } else {
                    op.attrs
                        .insert("s_value".to_string(), AttrValue::Str(actual_spec_name));
                }
            }
        }

        // Add all newly created specializations to the module.
        module.functions.extend(new_functions);
    }

    total_specs
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirModule;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_module(functions: Vec<TirFunction>) -> TirModule {
        TirModule {
            name: "test".to_string(),
            functions,
            class_hierarchy: None,
        }
    }

    /// Build a generic two-param function (DynBox, DynBox) → DynBox with a
    /// single Add op and a Return.
    fn make_generic_add() -> TirFunction {
        let mut func = TirFunction::new(
            "add".to_string(),
            vec![TirType::DynBox, TirType::DynBox],
            TirType::DynBox,
        );
        let result_id = ValueId(func.next_value);
        func.next_value += 1;
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result_id],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result_id],
        };
        func
    }

    /// Build a caller function that calls `callee_name` with two arguments of
    /// the given types.
    #[allow(dead_code)]
    fn make_caller(callee_name: &str, arg_ty_a: TirType, arg_ty_b: TirType) -> TirFunction {
        let mut func = TirFunction::new("main".to_string(), vec![], TirType::DynBox);

        // Allocate two const-int results to serve as typed arguments.
        let a = ValueId(func.next_value);
        func.next_value += 1;
        let b = ValueId(func.next_value);
        func.next_value += 1;
        let call_result = ValueId(func.next_value);
        func.next_value += 1;

        let mut const_a_attrs = AttrDict::new();
        const_a_attrs.insert("value".into(), AttrValue::Int(1));
        let mut const_b_attrs = AttrDict::new();
        const_b_attrs.insert("value".into(), AttrValue::Int(2));

        // Determine opcode for the const op based on arg type (approximation for test).
        let const_opcode_a = if matches!(arg_ty_a, TirType::F64) {
            OpCode::ConstFloat
        } else if matches!(arg_ty_a, TirType::Str) {
            OpCode::ConstStr
        } else {
            OpCode::ConstInt
        };
        let const_opcode_b = if matches!(arg_ty_b, TirType::F64) {
            OpCode::ConstFloat
        } else if matches!(arg_ty_b, TirType::Str) {
            OpCode::ConstStr
        } else {
            OpCode::ConstInt
        };

        let mut call_attrs = AttrDict::new();
        call_attrs.insert("callee".into(), AttrValue::Str(callee_name.to_string()));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: const_opcode_a,
            operands: vec![],
            results: vec![a],
            attrs: const_a_attrs,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: const_opcode_b,
            operands: vec![],
            results: vec![b],
            attrs: const_b_attrs,
            source_span: None,
        });

        // Manually set the types of a and b in the block args / simulate typed values.
        // We do this by adding a TypeGuard-like op that yields the concrete type.
        // For the test, we instead use a simpler approach: store typed block args via
        // TirValue directly — but since we're using ConstInt ops, type_refine will
        // give them I64/F64 types automatically. We'll use the const result ValueIds
        // directly as call operands.

        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: vec![a, b],
            results: vec![call_result],
            attrs: call_attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![call_result],
        };

        func
    }

    /// Build a caller that passes already-typed block arguments (to ensure
    /// type env sees non-DynBox types without relying on type_refine inference).
    fn make_caller_with_typed_args(callee_name: &str, arg_tys: Vec<TirType>) -> TirFunction {
        let mut func = TirFunction::new("main".to_string(), arg_tys.clone(), TirType::DynBox);
        // arg ValueIds are 0..arg_tys.len()
        let call_result = ValueId(func.next_value);
        func.next_value += 1;

        let operands: Vec<ValueId> = (0..arg_tys.len() as u32).map(ValueId).collect();
        let mut call_attrs = AttrDict::new();
        call_attrs.insert("callee".into(), AttrValue::Str(callee_name.to_string()));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands,
            results: vec![call_result],
            attrs: call_attrs,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![call_result],
        };
        func
    }

    // -----------------------------------------------------------------------
    // Test 1: (I64, I64) call → specialization created
    // -----------------------------------------------------------------------
    #[test]
    fn specializes_i64_i64_call() {
        let generic = make_generic_add();
        let caller = make_caller_with_typed_args("add", vec![TirType::I64, TirType::I64]);
        let mut module = make_module(vec![generic, caller]);

        let specs = monomorphize(&mut module);

        assert_eq!(specs, 1, "expected exactly 1 specialization");
        let names: Vec<&str> = module.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(
            names.contains(&"add__I64_I64"),
            "specialized function not found; got: {:?}",
            names
        );

        // The specialized function should have I64 param types.
        let spec = module
            .functions
            .iter()
            .find(|f| f.name == "add__I64_I64")
            .unwrap();
        assert_eq!(spec.param_types, vec![TirType::I64, TirType::I64]);
    }

    // -----------------------------------------------------------------------
    // Test 2: (F64, F64) call → second independent specialization
    // -----------------------------------------------------------------------
    #[test]
    fn specializes_f64_f64_call() {
        let generic = make_generic_add();
        let caller_i64 = make_caller_with_typed_args("add", vec![TirType::I64, TirType::I64]);
        let caller_f64 = make_caller_with_typed_args("add", vec![TirType::F64, TirType::F64]);

        // Use separate callers in separate "main" functions (rename for module validity).
        let mut caller_f64_renamed = caller_f64;
        caller_f64_renamed.name = "main2".to_string();

        let mut module = make_module(vec![generic, caller_i64, caller_f64_renamed]);

        let specs = monomorphize(&mut module);

        assert_eq!(specs, 2, "expected 2 specializations (I64 and F64)");
        let names: Vec<&str> = module.functions.iter().map(|f| f.name.as_str()).collect();
        assert!(
            names.contains(&"add__I64_I64"),
            "missing I64 specialization"
        );
        assert!(
            names.contains(&"add__F64_F64"),
            "missing F64 specialization"
        );
    }

    // -----------------------------------------------------------------------
    // Test 3: (DynBox, I64) call → NOT specialized
    // -----------------------------------------------------------------------
    #[test]
    fn does_not_specialize_partial_dynbox() {
        let generic = make_generic_add();
        let caller = make_caller_with_typed_args("add", vec![TirType::DynBox, TirType::I64]);
        let mut module = make_module(vec![generic, caller]);

        let specs = monomorphize(&mut module);

        assert_eq!(specs, 0, "should not specialize when any arg is DynBox");
        let names: Vec<&str> = module.functions.iter().map(|f| f.name.as_str()).collect();
        // No specialized copy should exist.
        assert!(
            !names.iter().any(|n| n.starts_with("add__")),
            "unexpected specialization found: {:?}",
            names
        );
    }

    // -----------------------------------------------------------------------
    // Test 4: Cache prevents duplicate specializations
    // -----------------------------------------------------------------------
    #[test]
    fn cache_prevents_duplicates() {
        let generic = make_generic_add();
        // Two callers with identical (I64, I64) types.
        let caller_a = make_caller_with_typed_args("add", vec![TirType::I64, TirType::I64]);
        let mut caller_b = make_caller_with_typed_args("add", vec![TirType::I64, TirType::I64]);
        caller_b.name = "main2".to_string();

        let mut module = make_module(vec![generic, caller_a, caller_b]);

        let specs = monomorphize(&mut module);

        // Only one specialization should be created despite two call sites.
        assert_eq!(
            specs, 1,
            "cache should deduplicate identical specializations"
        );
        let count = module
            .functions
            .iter()
            .filter(|f| f.name == "add__I64_I64")
            .count();
        assert_eq!(count, 1, "should have exactly one add__I64_I64 function");
    }

    // -----------------------------------------------------------------------
    // Test 5: Already-concrete function (no DynBox) is not re-specialized
    // -----------------------------------------------------------------------
    #[test]
    fn no_dynbox_params_not_specialized() {
        // A function that already has concrete params.
        let concrete = TirFunction::new(
            "add_concrete".to_string(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
        );
        let caller = make_caller_with_typed_args("add_concrete", vec![TirType::I64, TirType::I64]);
        let mut module = make_module(vec![concrete, caller]);

        let specs = monomorphize(&mut module);

        assert_eq!(
            specs, 0,
            "no specialization needed for already-concrete function"
        );
    }

    // -----------------------------------------------------------------------
    // Test 6: Call site attr is rewritten to specialized name
    // -----------------------------------------------------------------------
    #[test]
    fn call_site_rewritten_to_specialized_name() {
        let generic = make_generic_add();
        let caller = make_caller_with_typed_args("add", vec![TirType::I64, TirType::I64]);
        let mut module = make_module(vec![generic, caller]);

        monomorphize(&mut module);

        // Find the "main" function and verify its call op was rewritten.
        let main_func = module.functions.iter().find(|f| f.name == "main").unwrap();
        let entry = &main_func.blocks[&main_func.entry_block];
        let call_op = entry
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::Call)
            .unwrap();
        let target = call_op
            .attrs
            .get("callee")
            .or_else(|| call_op.attrs.get("s_value"))
            .and_then(|v| {
                if let AttrValue::Str(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            });
        assert_eq!(
            target,
            Some("add__I64_I64"),
            "call site should be rewritten to specialized name"
        );
    }

    // -----------------------------------------------------------------------
    // Test 7: Empty module — no crash
    // -----------------------------------------------------------------------
    #[test]
    fn empty_module_no_crash() {
        let mut module = make_module(vec![]);
        let specs = monomorphize(&mut module);
        assert_eq!(specs, 0);
    }
}
