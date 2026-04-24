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

/// Cache key for call-site-sensitive monomorphization.
///
/// The key is `(caller_function_name, call_site_index, callee_function_name,
/// concrete_arg_types)`. By including the caller and call site index, the same
/// function called from two sites with different type contexts gets two
/// specialized copies. This eliminates intra-callee type guards when the
/// caller has already proven the types (GraalVM Truffle partial-evaluation
/// approach).
///
/// When the caller is unknown or cross-module, `caller_func` is empty and
/// `call_site_index` is 0, which degenerates to the old `(func_name,
/// arg_types)` key behavior.
type SpecCacheKey = (String, usize, String, Vec<TirType>);

/// Cache: SpecCacheKey → specialized_function_name.
type SpecCache = HashMap<SpecCacheKey, String>;

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
        loop_cond_blocks: func.loop_cond_blocks.clone(),
    }
}

/// Build a call-site-sensitive mangled name.
///
/// The name includes the caller function and call-site index so that
/// two sites calling the same callee with the same arg types but
/// different type-proof contexts produce distinct specializations.
/// This is the GraalVM Truffle strategy: partial-evaluate per call
/// site, not per global type tuple.
fn mangle_callsite_name(
    callee_name: &str,
    caller_func: &str,
    call_site_idx: usize,
    concrete_args: &[TirType],
) -> String {
    let type_suffix: String = concrete_args
        .iter()
        .map(mangle_type)
        .collect::<Vec<_>>()
        .join("_");
    if caller_func.is_empty() {
        // Degenerate case: no caller context.
        format!("{}__{}", callee_name, type_suffix)
    } else {
        format!(
            "{}__{}_{}_{}",
            callee_name, caller_func, call_site_idx, type_suffix
        )
    }
}

/// Specialize a clone of `callee` for the given concrete argument types
/// at a specific call site.
///
/// 1. Clones the function under a call-site-sensitive mangled name.
/// 2. Replaces entry block argument types with the concrete types.
/// 3. Updates `param_types` to match.
/// 4. Runs `type_refine::refine_types` to propagate concrete types through
///    all operations.
///
/// The `caller_func` and `call_site_idx` are embedded in the mangled name
/// so that two different call sites produce distinct specializations even
/// with identical arg types. This enables the callee body to be optimized
/// against the caller's type proof context (eliminating redundant guards).
fn specialize(
    callee: &TirFunction,
    concrete_args: &[TirType],
    caller_func: &str,
    call_site_idx: usize,
) -> TirFunction {
    let specialized_name =
        mangle_callsite_name(&callee.name, caller_func, call_site_idx, concrete_args);

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

/// Scan all ops in a function and collect call-site-sensitive specialization
/// candidates.
///
/// Returns: Vec<(callee_name, arg_types, block_id, op_index, call_site_index)>
///
/// The `call_site_index` is a sequential counter per caller function,
/// monotonically increasing across blocks and ops. This index, combined
/// with the caller's function name, forms the call-site-sensitive
/// monomorphization key.
fn collect_call_sites(
    func: &TirFunction,
    env: &HashMap<ValueId, TirType>,
) -> Vec<(String, Vec<TirType>, BlockId, usize, usize)> {
    let mut sites = Vec::new();
    let mut call_site_counter: usize = 0;

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

            let current_site = call_site_counter;
            call_site_counter += 1;

            // Resolve argument types from the type env.
            let arg_types: Vec<TirType> = op
                .operands
                .iter()
                .map(|id| env.get(id).cloned().unwrap_or(TirType::DynBox))
                .collect();

            // Only specialize when ALL args are concrete.
            if all_concrete(&arg_types) && !arg_types.is_empty() {
                sites.push((callee, arg_types, bid, op_idx, current_site));
            }
        }
    }

    sites
}

/// Core monomorphization driver with call-site-sensitive specialization.
///
/// This implements the GraalVM Truffle partial-evaluation strategy: the
/// monomorphization key is `(caller_func, call_site_index, callee_name,
/// arg_types)` rather than just `(callee_name, arg_types)`. Two call
/// sites in different callers (or different locations within the same
/// caller) that call the same function with the same arg types produce
/// independent specializations. This lets type refinement within each
/// copy exploit the caller's proven type context, eliminating guards
/// that would be needed in a single shared specialization.
///
/// Performs up to `MAX_DEPTH` rounds of specialization. Each round:
/// 1. Scans all existing functions for call sites with concrete arg types.
/// 2. For each new `(caller, site_idx, callee, arg_types)` not in cache:
///    a. Clones and specializes the callee.
///    b. Adds the specialization to the module.
///    c. Records the mapping in the cache.
/// 3. Rewrites all call sites to target the specialized name.
///
/// Returns the total number of specializations created.
pub fn monomorphize(module: &mut TirModule) -> usize {
    const MAX_TOTAL_SPECS: usize = 500;
    let mut cache: SpecCache = HashMap::new();
    let mut total_specs = 0;

    for _depth in 0..MAX_DEPTH {
        if module.functions.len() > MAX_TOTAL_SPECS {
            break; // Hard cap to prevent OOM from polymorphic recursion.
        }
        // Collect all pending specializations this round.
        // Tuple: (caller_func_idx, block_id, op_idx, callee_name, specialized_name,
        //         arg_types, caller_name, call_site_index)
        let mut pending: Vec<(usize, BlockId, usize, String, String, Vec<TirType>, String, usize)> =
            Vec::new();

        // Build the function name → index map for fast lookups.
        let func_index: HashMap<String, usize> = module
            .functions
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.clone(), i))
            .collect();

        // Scan every function for specializable call sites.
        for caller_idx in 0..module.functions.len() {
            let caller_name = module.functions[caller_idx].name.clone();
            let env = build_type_env(&module.functions[caller_idx]);
            let sites = collect_call_sites(&module.functions[caller_idx], &env);

            for (callee_name, arg_types, bid, op_idx, call_site_idx) in sites {
                // Skip if callee is not defined in this module.
                if !func_index.contains_key(&callee_name) {
                    continue;
                }

                let cache_key = (
                    caller_name.clone(),
                    call_site_idx,
                    callee_name.clone(),
                    arg_types.clone(),
                );
                let spec_name = if let Some(existing) = cache.get(&cache_key) {
                    existing.clone()
                } else {
                    mangle_callsite_name(
                        &callee_name,
                        &caller_name,
                        call_site_idx,
                        &arg_types,
                    )
                };

                // Skip if already resolved to itself (no DynBox params to specialize).
                if spec_name == callee_name {
                    continue;
                }

                pending.push((
                    caller_idx,
                    bid,
                    op_idx,
                    callee_name,
                    spec_name,
                    arg_types,
                    caller_name.clone(),
                    call_site_idx,
                ));
            }
        }

        if pending.is_empty() {
            break;
        }

        // Process pending specializations — create new functions, rewrite call sites.
        let mut new_functions: Vec<TirFunction> = Vec::new();

        for (caller_idx, bid, op_idx, callee_name, spec_name, arg_types, caller_name, call_site_idx) in
            &pending
        {
            let cache_key = (
                caller_name.clone(),
                *call_site_idx,
                callee_name.clone(),
                arg_types.clone(),
            );

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
                        let specialized =
                            specialize(callee, arg_types, caller_name, *call_site_idx);
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
    // Test 1: (I64, I64) call → specialization created (call-site-sensitive)
    // -----------------------------------------------------------------------
    #[test]
    fn specializes_i64_i64_call() {
        let generic = make_generic_add();
        let caller = make_caller_with_typed_args("add", vec![TirType::I64, TirType::I64]);
        let mut module = make_module(vec![generic, caller]);

        let specs = monomorphize(&mut module);

        assert_eq!(specs, 1, "expected exactly 1 specialization");
        // Call-site-sensitive: name includes caller context.
        let spec = module
            .functions
            .iter()
            .find(|f| f.name.starts_with("add__") && f.param_types == vec![TirType::I64, TirType::I64])
            .expect("specialized function with I64 params not found");
        assert!(
            spec.name.contains("main"),
            "call-site-sensitive name should include caller 'main'; got: {}",
            spec.name
        );
    }

    // -----------------------------------------------------------------------
    // Test 2: (F64, F64) call → second independent specialization
    // -----------------------------------------------------------------------
    #[test]
    fn specializes_f64_f64_call() {
        let generic = make_generic_add();
        let caller_i64 = make_caller_with_typed_args("add", vec![TirType::I64, TirType::I64]);
        let mut caller_f64 = make_caller_with_typed_args("add", vec![TirType::F64, TirType::F64]);
        caller_f64.name = "main2".to_string();

        let mut module = make_module(vec![generic, caller_i64, caller_f64]);

        let specs = monomorphize(&mut module);

        assert_eq!(specs, 2, "expected 2 specializations (I64 and F64)");
        let i64_spec = module
            .functions
            .iter()
            .any(|f| f.name.starts_with("add__") && f.param_types == vec![TirType::I64, TirType::I64]);
        let f64_spec = module
            .functions
            .iter()
            .any(|f| f.name.starts_with("add__") && f.param_types == vec![TirType::F64, TirType::F64]);
        assert!(i64_spec, "missing I64 specialization");
        assert!(f64_spec, "missing F64 specialization");
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
    // Test 4: Call-site-sensitive cache produces per-caller specializations
    // -----------------------------------------------------------------------
    #[test]
    fn callsite_sensitive_distinct_callers() {
        let generic = make_generic_add();
        // Two callers with identical (I64, I64) types from different functions.
        let caller_a = make_caller_with_typed_args("add", vec![TirType::I64, TirType::I64]);
        let mut caller_b = make_caller_with_typed_args("add", vec![TirType::I64, TirType::I64]);
        caller_b.name = "main2".to_string();

        let mut module = make_module(vec![generic, caller_a, caller_b]);

        let specs = monomorphize(&mut module);

        // Call-site-sensitive: two different callers produce two distinct
        // specializations (each optimized for its caller's context).
        assert_eq!(
            specs, 2,
            "call-site-sensitive should produce per-caller specializations"
        );
        let spec_names: Vec<&str> = module
            .functions
            .iter()
            .filter(|f| f.name.starts_with("add__"))
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(spec_names.len(), 2, "expected 2 distinct specializations; got: {:?}", spec_names);
        // Both should have I64 params.
        for spec_name in &spec_names {
            let func = module.functions.iter().find(|f| f.name == *spec_name).unwrap();
            assert_eq!(func.param_types, vec![TirType::I64, TirType::I64]);
        }
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
        // Call-site-sensitive name includes caller context.
        let target_str = target.expect("call site should have a target name");
        assert!(
            target_str.starts_with("add__") && target_str.contains("main"),
            "call site should be rewritten to call-site-sensitive name; got: {}",
            target_str
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

    // -----------------------------------------------------------------------
    // Test 8: Same caller, two call sites with same types → two specializations
    // -----------------------------------------------------------------------
    #[test]
    fn same_caller_two_sites_two_specs() {
        let generic = make_generic_add();
        // Build a caller that calls `add` twice (two call sites, same arg types).
        let mut func = TirFunction::new("main".to_string(), vec![TirType::I64, TirType::I64], TirType::DynBox);
        let call_result_1 = ValueId(func.next_value);
        func.next_value += 1;
        let call_result_2 = ValueId(func.next_value);
        func.next_value += 1;

        let operands: Vec<ValueId> = vec![ValueId(0), ValueId(1)];
        let mut call_attrs_1 = AttrDict::new();
        call_attrs_1.insert("callee".into(), AttrValue::Str("add".to_string()));
        let mut call_attrs_2 = AttrDict::new();
        call_attrs_2.insert("callee".into(), AttrValue::Str("add".to_string()));

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands: operands.clone(),
            results: vec![call_result_1],
            attrs: call_attrs_1,
            source_span: None,
        });
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Call,
            operands,
            results: vec![call_result_2],
            attrs: call_attrs_2,
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![call_result_2],
        };

        let mut module = make_module(vec![generic, func]);
        let specs = monomorphize(&mut module);

        // Two distinct call sites → two specializations.
        assert_eq!(specs, 2, "two call sites in same caller should produce two specializations");
    }
}
