//! SimpleIR → TIR construction pipeline.
//!
//! Chains together CFG extraction, SSA conversion, and TIR function assembly
//! into a single `lower_to_tir` entry point.

mod loop_structure;
mod pre_ssa;
mod type_inference;

use self::loop_structure::{detect_loop_cond_blocks, detect_loop_structure};
use self::pre_ssa::{rewrite_cell_locals_to_store_load, rewrite_loop_index_to_store_load};
#[cfg(test)]
use self::type_inference::string_to_tir_type;
use self::type_inference::{
    infer_return_type, param_string_to_tir_type, propagate_arithmetic_types,
};
use std::collections::HashMap;

use crate::ir::FunctionIR;

use super::blocks::{BlockId, TirBlock};
use super::cfg::CFG;
use super::function::{TirFunction, TirModule};
use super::op_kinds_generated::opcode_sets_exception_handling_table;
use super::ssa::{SsaOutput, convert_to_ssa_with_name_and_params};
use super::types::TirType;

/// Lift every **non-extern** `FunctionIR` in `functions` to TIR and assemble a
/// [`TirModule`] for the whole-program module phase (the E1 inliner). Returns the
/// module plus an `idx_map` aligning each module position to its original index
/// in `functions` — externs are skipped (their bodies live in `stdlib_shared.o`
/// and carry no inlinable ops), so module positions are NOT equal to source
/// indices. The caller back-converts each post-inline `TirFunction` at module
/// position `p` into `functions[idx_map[p]]`.
///
/// Mirrors the extern filter the legacy `compute_leaf_functions_via_call_graph`
/// used (`.filter(|f| !f.is_extern)`), so the call graph the inliner builds over
/// this module sees exactly the local function bodies.
pub fn lower_functions_to_tir_module(functions: &[FunctionIR]) -> (TirModule, Vec<usize>) {
    let mut tir_functions = Vec::new();
    let mut idx_map = Vec::new();
    for (i, f) in functions.iter().enumerate() {
        if f.is_extern {
            continue;
        }
        tir_functions.push(lower_to_tir(f));
        idx_map.push(i);
    }
    (
        TirModule {
            name: "native_module".to_string(),
            functions: tir_functions,
        },
        idx_map,
    )
}

/// Convert a SimpleIR function into a fully-constructed TIR function.
///
/// Pipeline: SimpleIR ops → CFG extraction → SSA conversion → TIR construction.
///
/// TIR typing must come from structural sources only: explicit function
/// parameter types plus canonical propagation over the SSA graph. Transport
/// compatibility metadata on SimpleIR is intentionally ignored here.
pub fn lower_to_tir(ir: &FunctionIR) -> TirFunction {
    if std::env::var("MOLT_TRACE_SIMPLE_IMPORT").as_deref() == Ok("1") {
        for op in &ir.ops {
            if op.kind.contains("import") {
                eprintln!(
                    "Simple import trace: func={} kind={} args={:?} var={:?} out={:?} s_value={:?}",
                    ir.name, op.kind, op.args, op.var, op.out, op.s_value
                );
            }
        }
    }
    // 0. Memory SSA: rewrite cell-based local variables (store_index/index on
    //    the locals list) into store_var/load_var. This enables the SSA pass
    //    to track local variable mutations through loop iterations — the key
    //    enabler for type specialization and integer-lane optimization on loops.
    //
    //    The rewrite is safe because lower_to_simple_ir restores the original
    //    store_index/index patterns from the SSA output.
    // Rewrite loop_index_start/loop_index_next to store_var/load_var so the
    // SSA pass creates proper phi nodes at loop headers for induction variables.
    let rewritten_ops = rewrite_loop_index_to_store_load(&ir.ops);
    let mut working_ops = if rewritten_ops.is_empty() {
        ir.ops.clone()
    } else {
        rewritten_ops
    };
    // RC drop-insertion substrate (design 20): function-level attrs do not live
    // in FunctionIR, so drop facts round-trip as leading SimpleIR marker ops. The
    // full `drop_inserted` marker tells native to disable its legacy value-tracker
    // because TIR owns the whole function's RC. The narrower exception-region
    // marker only protects already-inserted CreationRef/MatchRef releases across
    // relifts and optimizer re-runs; native deliberately ignores it as an RC
    // suppression signal. Both markers carry no per-op TIR semantics, so strip
    // them before CFG/SSA construction and preserve them as function attrs.
    let had_drop_inserted_marker = working_ops
        .iter()
        .any(|op| op.kind == crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR);
    let had_exception_region_drops_marker = working_ops.iter().any(|op| {
        op.kind == crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR
    });
    working_ops.retain(|op| {
        op.kind != crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR
            && op.kind != crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR
    });
    // Memory SSA: rewrite cell-based locals (store_index/index on a 1-elem
    // list "cell") to store_var/load_var so SSA generates proper phi nodes
    // at loop headers for cell variables. Always-on; no env gate.
    let _cell_rewrite_applied = rewrite_cell_locals_to_store_load(&mut working_ops);

    let tmp_ir = crate::ir::FunctionIR {
        name: ir.name.clone(),
        ops: working_ops.clone(),
        params: ir.params.clone(),
        param_types: ir.param_types.clone(),
        source_file: ir.source_file.clone(),
        is_extern: false,
    };
    let ir_ref = &tmp_ir;
    let ops = &working_ops[..];

    // 1. Build CFG from the rewritten op stream.
    let cfg = CFG::build(ops);

    // 2. Convert to SSA with block arguments (pass params for implicit entry defs).
    // No catch_unwind — panics propagate cleanly through rayon. Using
    // AssertUnwindSafe on borrowed state violates Rust's unwind safety contract.
    let ssa = convert_to_ssa_with_name_and_params(&ir.name, &cfg, ops, &ir.params);

    // 3. Assemble the TirFunction from the SSA output.
    let mut tir_func = assemble_function(ir_ref, &cfg, ssa);
    // Preserve the RC drop-insertion marker across the round-trip (see above).
    if had_drop_inserted_marker {
        tir_func.attrs.insert(
            crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR.to_string(),
            crate::tir::ops::AttrValue::Bool(true),
        );
    }
    if had_exception_region_drops_marker {
        tir_func.attrs.insert(
            crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR.to_string(),
            crate::tir::ops::AttrValue::Bool(true),
        );
    }
    tir_func
}

/// Assemble a `TirFunction` from a `FunctionIR`, its `CFG`, and the `SsaOutput`.
fn assemble_function(ir: &FunctionIR, cfg: &CFG, ssa: SsaOutput) -> TirFunction {
    let SsaOutput {
        blocks: mut tir_blocks,
        mut types,
        next_value,
    } = ssa;

    // Determine semantic parameter types. `param_types` also carries the
    // native ABI carrier marker `i64` for boxed Molt object words; that marker
    // is not a Python `int` proof and must remain DynBox in TIR.
    let param_types: Vec<TirType> = if let Some(ref pt) = ir.param_types {
        pt.iter().map(|s| param_string_to_tir_type(s)).collect()
    } else {
        ir.params.iter().map(|_| TirType::DynBox).collect()
    };

    // Propagate parameter types to the entry block arguments in the types map.
    // This is critical for SCCP: without it, parameters default to DynBox and
    // the type inference can't prove that `n + 1` produces I64 even when
    // the function signature says `n: int`. Entry block args correspond 1:1
    // to function parameters.
    if let Some(entry) = tir_blocks.first() {
        for (arg_val, param_ty) in entry.args.iter().zip(param_types.iter()) {
            if *param_ty != TirType::DynBox {
                types.insert(arg_val.id, param_ty.clone());
            }
        }
    }
    if let Some(entry) = tir_blocks.first_mut() {
        for (arg_val, param_ty) in entry.args.iter_mut().zip(param_types.iter()) {
            if *param_ty != TirType::DynBox {
                arg_val.ty = param_ty.clone();
            }
        }
    }

    // Forward type propagation: when all operands of an Add/Sub/Mul/etc. are
    // known-typed from constants or parameter signatures, infer the result
    // type before deriving the function return contract.
    propagate_arithmetic_types(&tir_blocks, &mut types);

    // Infer a return type from the SSA output by inspecting return terminators.
    let return_type = infer_return_type(&tir_blocks, &types);

    // Build the block map keyed by BlockId.
    let mut block_map: HashMap<BlockId, TirBlock> = HashMap::with_capacity(tir_blocks.len());
    for block in tir_blocks {
        block_map.insert(block.id, block);
    }

    let entry_block = if cfg.blocks.is_empty() {
        BlockId(0)
    } else {
        BlockId(cfg.entry as u32)
    };

    let next_block = block_map.len() as u32;

    // Detect whether the function contains exception-handling ops.
    let has_exception_handling = block_map.values().any(|block| {
        block
            .ops
            .iter()
            .any(|op| opcode_sets_exception_handling_table(op.opcode))
    });

    // Build label_id_map: for each CFG block that starts with a label/state_label,
    // record the original label value so the back-conversion can emit labels
    // with matching IDs for check_exception / jump / br_if targets.
    let mut label_id_map = HashMap::new();
    for (bid, bb) in cfg.blocks.iter().enumerate() {
        // Scan the ops in this block for a leading label/state_label.
        for op_idx in bb.start_op..bb.end_op {
            let op = &ir.ops[op_idx];
            if matches!(op.kind.as_str(), "label" | "state_label") {
                if let Some(label_val) = op.value {
                    label_id_map.insert(bid as u32, label_val);
                }
                break; // Only care about the first label in the block.
            }
            // If we hit a non-structural op before finding a label, stop.
            if !is_structural(&op.kind) {
                break;
            }
        }
    }

    // Detect loop structural roles from the original SimpleIR ops.
    let (loop_roles, loop_pairs, loop_break_kinds) = detect_loop_structure(ir, cfg);
    let loop_cond_blocks = detect_loop_cond_blocks(ir, cfg);

    TirFunction {
        name: ir.name.clone(),
        param_names: ir.params.clone(),
        param_types,
        return_type,
        blocks: block_map,
        entry_block,
        next_value,
        next_block,
        attrs: {
            let mut a = super::ops::AttrDict::new();
            if ir.ops.iter().any(|op| op.kind == "ret") {
                a.insert(
                    "_original_has_ret".into(),
                    super::ops::AttrValue::Bool(true),
                );
            }
            if let Some(source_file) = &ir.source_file
                && !source_file.is_empty()
            {
                a.insert(
                    super::ops::SOURCE_FILE_ATTR.into(),
                    super::ops::AttrValue::Str(source_file.clone()),
                );
            }
            a
        },
        value_types: types,
        has_exception_handling,
        label_id_map,
        loop_roles,
        loop_pairs,
        loop_break_kinds,
        loop_cond_blocks,
    }
}

// Use shared is_structural from parent module (ensures SSA and lower_from_simple
// always agree on which ops to skip).
use super::is_structural;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FunctionIR, OpIR};
    use crate::tir::blocks::Terminator;
    use crate::tir::ops::OpCode;
    use crate::tir::types::TirType;

    /// Helper: build a FunctionIR with given name, params, and ops.
    fn make_func(name: &str, params: &[&str], ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            name: name.to_string(),
            params: params.iter().map(|s| s.to_string()).collect(),
            ops,
            param_types: None,
            source_file: None,
            is_extern: false,
        }
    }

    #[test]
    fn lower_functions_to_tir_module_skips_externs_and_aligns_idx() {
        // [non-extern "a", extern "ext", non-extern "b"] → module has {a, b}
        // (extern skipped), idx_map aligns module position → original index.
        let mut ext = make_func("ext", &[], vec![op("ret_void")]);
        ext.is_extern = true;
        let funcs = vec![
            make_func("a", &[], vec![op("ret_void")]),
            ext,
            make_func("b", &[], vec![op("ret_void")]),
        ];
        let (module, idx_map) = lower_functions_to_tir_module(&funcs);
        assert_eq!(module.functions.len(), 2, "externs are skipped");
        assert_eq!(idx_map, vec![0, 2], "module position maps to source index");
        assert_eq!(module.functions[0].name, "a");
        assert_eq!(module.functions[1].name, "b");
    }

    /// Helper to create an `OpIR` with just a `kind`.
    fn op(kind: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind`, `value`, and `out`.
    fn op_val_out(kind: &str, value: i64, out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            value: Some(value),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind`, `args`, and `out`.
    fn op_args_out(kind: &str, args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            out: Some(out.to_string()),
            ..OpIR::default()
        }
    }

    /// Helper to create an `OpIR` with `kind` and `args`.
    fn op_args(kind: &str, args: &[&str]) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            ..OpIR::default()
        }
    }

    /// Helper: create an op with integer compatibility hint.
    fn op_fast_int(kind: &str, args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            out: Some(out.to_string()),
            fast_int: Some(true),
            ..OpIR::default()
        }
    }

    /// Helper: create an op with float compatibility hint.
    fn op_fast_float(kind: &str, args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            out: Some(out.to_string()),
            fast_float: Some(true),
            ..OpIR::default()
        }
    }

    #[test]
    fn cell_rewrite_skips_cells_escaped_into_closure_tuple() {
        let mut ops = vec![
            op_args_out("missing", &[], "missing"),
            op_args_out("list_new", &["missing"], "cell"),
            op_val_out("const", 0, "zero"),
            op_val_out("const", 7, "value"),
            op_args("store_index", &["cell", "zero", "value"]),
            op_args_out("tuple_new", &["cell"], "closure"),
            op_args_out("index", &["cell", "zero"], "loaded"),
        ];

        assert!(!rewrite_cell_locals_to_store_load(&mut ops));
        assert_eq!(ops[4].kind, "store_index");
        assert_eq!(ops[6].kind, "index");
    }

    #[test]
    fn cell_rewrite_handles_multiple_unescaped_cells_independently() {
        let mut ops = vec![
            op_args_out("missing", &[], "missing_a"),
            op_args_out("list_new", &["missing_a"], "cell_a"),
            op_args_out("missing", &[], "missing_b"),
            op_args_out("list_new", &["missing_b"], "cell_b"),
            op_val_out("const", 0, "zero"),
            op_val_out("const", 1, "value_a"),
            op_args("store_index", &["cell_a", "zero", "value_a"]),
            op_args_out("index", &["cell_a", "zero"], "loaded_a"),
            op_val_out("const", 2, "value_b"),
            op_args("store_index", &["cell_b", "zero", "value_b"]),
            op_args_out("index", &["cell_b", "zero"], "loaded_b"),
        ];

        assert!(rewrite_cell_locals_to_store_load(&mut ops));
        assert_eq!(ops[6].kind, "store_var");
        assert_eq!(ops[6].var.as_deref(), Some("_cell_cell_a_0"));
        assert_eq!(ops[7].kind, "load_var");
        assert_eq!(ops[7].var.as_deref(), Some("_cell_cell_a_0"));
        assert_eq!(ops[9].kind, "store_var");
        assert_eq!(ops[9].var.as_deref(), Some("_cell_cell_b_0"));
        assert_eq!(ops[10].kind, "load_var");
        assert_eq!(ops[10].var.as_deref(), Some("_cell_cell_b_0"));
    }

    // =======================================================================
    // Test 1: Trivial function — const + add + ret
    // =======================================================================
    #[test]
    fn trivial_function_lowering() {
        let func_ir = make_func(
            "test_add",
            &[],
            vec![
                op_val_out("const", 1, "x"),
                op_args_out("add", &["x"], "y"),
                op_args("ret", &["y"]),
            ],
        );

        let tir = lower_to_tir(&func_ir);

        assert_eq!(tir.name, "test_add");
        assert!(!tir.blocks.is_empty(), "should have at least one block");
        assert!(tir.blocks.contains_key(&tir.entry_block));

        // Should have exactly 1 block for straight-line code.
        assert_eq!(tir.blocks.len(), 1);

        // Entry block should have 2 ops (const + add; ret is structural).
        let entry = &tir.blocks[&tir.entry_block];
        // 3 ops: ConstNone (SSA undef sentinel) + ConstInt + Add; ret is structural.
        assert_eq!(
            entry.ops.len(),
            3,
            "entry should have undef sentinel, const, and add ops"
        );

        // Terminator should be Return.
        assert!(
            matches!(entry.terminator, Terminator::Return { .. }),
            "expected Return terminator, got {:?}",
            entry.terminator
        );
    }

    // =======================================================================
    // Test 2: Function with if/else control flow
    // =======================================================================
    #[test]
    fn if_else_control_flow() {
        let func_ir = make_func(
            "test_branch",
            &[],
            vec![
                op_val_out("const", 0, "c"), // 0 entry
                op_args("if", &["c"]),       // 1 ends entry
                op_val_out("const", 1, "x"), // 2 then
                op("else"),                  // 3 else
                op_val_out("const", 2, "x"), // 4 else body
                op("end_if"),                // 5 join
                op_args("ret", &["x"]),      // 6 return
            ],
        );

        let tir = lower_to_tir(&func_ir);

        assert_eq!(tir.name, "test_branch");
        assert!(
            tir.blocks.len() >= 3,
            "if/else should produce at least 3 blocks"
        );

        // Find the join block — it should have a block argument for `x`.
        let join_block = tir.blocks.values().find(|b| !b.args.is_empty());
        assert!(
            join_block.is_some(),
            "should have a join block with block arguments"
        );
        let join = join_block.unwrap();
        assert_eq!(
            join.args.len(),
            1,
            "join block should have 1 block arg (for x)"
        );

        // There should be a block with a CondBranch terminator (the block
        // containing the `if` op — which may or may not be the entry block,
        // depending on how the CFG splits).
        let has_cond_branch = tir
            .blocks
            .values()
            .any(|b| matches!(b.terminator, Terminator::CondBranch { .. }));
        assert!(
            has_cond_branch,
            "should have a block with CondBranch terminator"
        );
    }

    #[test]
    fn module_import_preserves_operand_through_lower_to_tir() {
        let func_ir = make_func(
            "module_import_shape",
            &["__molt_module_obj__"],
            vec![
                OpIR {
                    kind: "line".to_string(),
                    value: Some(7),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("builtins".to_string()),
                    out: Some("v62".to_string()),
                    ..OpIR::default()
                },
                op_args_out("module_import", &["v62"], "v63"),
                OpIR {
                    kind: "check_exception".to_string(),
                    value: Some(3),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "const_str".to_string(),
                    s_value: Some("_builtins".to_string()),
                    out: Some("v64".to_string()),
                    ..OpIR::default()
                },
                OpIR {
                    kind: "module_set_attr".to_string(),
                    args: Some(vec![
                        "__molt_module_obj__".to_string(),
                        "v64".to_string(),
                        "v63".to_string(),
                    ]),
                    out: Some("none".to_string()),
                    ..OpIR::default()
                },
                op("ret_void"),
            ],
        );

        let tir = lower_to_tir(&func_ir);
        let import_op = tir
            .blocks
            .values()
            .flat_map(|block| block.ops.iter())
            .find(|op| op.opcode == crate::tir::ops::OpCode::Import)
            .expect("expected import op");
        assert_eq!(import_op.operands.len(), 1, "{:?}", import_op.operands);
    }

    #[test]
    fn gpu_thread_id_lowers_to_runtime_backed_call_in_tir() {
        let func_ir = make_func(
            "gpu_tid",
            &[],
            vec![
                OpIR {
                    kind: "gpu_thread_id".to_string(),
                    out: Some("tid".to_string()),
                    ..OpIR::default()
                },
                op_args("ret", &["tid"]),
            ],
        );

        let tir = lower_to_tir(&func_ir);
        let call_op = tir
            .blocks
            .values()
            .flat_map(|block| block.ops.iter())
            .find(|op| op.opcode == crate::tir::ops::OpCode::Call)
            .expect("expected gpu_thread_id to lower to a call op");
        assert_eq!(
            call_op.attrs.get("s_value"),
            Some(&crate::tir::ops::AttrValue::Str(
                "molt_gpu_thread_id".to_string()
            ))
        );
        assert_eq!(
            call_op.attrs.get("_original_kind"),
            Some(&crate::tir::ops::AttrValue::Str(
                "gpu_thread_id".to_string()
            ))
        );
    }

    // =======================================================================
    // Test 3: transport hints do not seed canonical SSA types
    // =======================================================================
    #[test]
    fn transport_hints_do_not_seed_canonical_types() {
        let func_ir = FunctionIR {
            name: "hint_only_add".into(),
            params: vec!["a".into(), "b".into(), "fa".into(), "fb".into()],
            ops: vec![
                op_fast_int("add", &["a", "b"], "c"),
                op_fast_float("mul", &["fa", "fb"], "fc"),
                op_args("ret", &["c"]),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func_ir);

        assert_eq!(
            tir.return_type,
            TirType::DynBox,
            "transport-only hints must not seed canonical TIR types"
        );
        for op in tir.blocks.values().flat_map(|block| &block.ops) {
            assert!(
                !op.attrs.contains_key("_fast_int"),
                "SimpleIR fast_int metadata must not enter TIR attrs: {op:?}"
            );
            assert!(
                !op.attrs.contains_key("_fast_float"),
                "SimpleIR fast_float metadata must not enter TIR attrs: {op:?}"
            );
        }
    }

    // =======================================================================
    // Test 4: Empty function
    // =======================================================================
    #[test]
    fn empty_function() {
        let func_ir = make_func("empty", &[], vec![]);
        let tir = lower_to_tir(&func_ir);

        assert_eq!(tir.name, "empty");
        // Empty ops → empty CFG → no blocks from SSA.
        assert!(tir.blocks.is_empty());
    }

    // =======================================================================
    // Test 5: Function with param_types annotation
    // =======================================================================
    #[test]
    fn param_types_from_annotation() {
        let func_ir = FunctionIR {
            name: "typed_add".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            ops: vec![op_args_out("add", &["a", "b"], "c"), op_args("ret", &["c"])],
            param_types: Some(vec!["int".to_string(), "float".to_string()]),
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func_ir);

        assert_eq!(tir.param_types.len(), 2);
        assert_eq!(tir.param_types[0], TirType::I64);
        assert_eq!(tir.param_types[1], TirType::F64);
        let entry = &tir.blocks[&tir.entry_block];
        assert_eq!(
            tir.value_types.get(&entry.args[0].id),
            Some(&TirType::I64),
            "entry param i64 fact must be present in the function-owned map"
        );
        assert_eq!(
            tir.value_types.get(&entry.args[1].id),
            Some(&TirType::F64),
            "entry param f64 fact must be present in the function-owned map"
        );
        let add_result = entry
            .ops
            .iter()
            .find(|op| op.opcode == OpCode::Add)
            .and_then(|op| op.results.first())
            .copied()
            .expect("typed add result");
        assert_eq!(
            tir.value_types.get(&add_result),
            Some(&TirType::F64),
            "arithmetic propagation must persist op-result facts on TirFunction"
        );
    }

    #[test]
    fn compound_param_types_from_annotation() {
        let func_ir = FunctionIR {
            name: "typed_container".to_string(),
            params: vec!["items".to_string()],
            ops: vec![op_args("ret", &["items"])],
            param_types: Some(vec!["list[int]".to_string()]),
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func_ir);
        let expected = TirType::List(Box::new(TirType::I64));

        assert_eq!(tir.param_types, vec![expected.clone()]);
        let entry = &tir.blocks[&tir.entry_block];
        assert_eq!(
            tir.value_types.get(&entry.args[0].id),
            Some(&expected),
            "entry param compound type fact must be present in the function-owned map"
        );
        assert_eq!(
            entry.args[0].ty, expected,
            "entry param argument must carry the structured compound type"
        );
    }

    #[test]
    fn abi_i64_param_type_is_not_a_semantic_int_fact() {
        let func_ir = FunctionIR {
            name: "boxed_carrier".to_string(),
            params: vec!["obj".to_string()],
            ops: vec![op_args("ret", &["obj"])],
            param_types: Some(vec!["i64".to_string()]),
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func_ir);

        assert_eq!(tir.param_types, vec![TirType::DynBox]);
        let entry = &tir.blocks[&tir.entry_block];
        assert_eq!(
            tir.value_types.get(&entry.args[0].id),
            Some(&TirType::DynBox),
            "native ABI carrier `i64` must stay a boxed dynamic value, not semantic I64"
        );
    }

    #[test]
    fn exception_region_drop_marker_round_trips_without_full_drop_gate() {
        let func_ir = FunctionIR {
            name: "exception_marker_transport".to_string(),
            params: vec![],
            ops: vec![
                op(crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
                op("ret_void"),
            ],
            param_types: None,
            source_file: None,
            is_extern: false,
        };

        let tir = lower_to_tir(&func_ir);

        assert!(matches!(
            tir.attrs
                .get(crate::tir::passes::drop_insertion::EXCEPTION_REGION_DROPS_INSERTED_ATTR),
            Some(crate::tir::ops::AttrValue::Bool(true))
        ));
        assert!(
            !tir.attrs
                .contains_key(crate::tir::passes::drop_insertion::DROP_INSERTED_ATTR),
            "exception-only marker must not be promoted to the native full-RC gate"
        );
        assert!(
            tir.blocks[&tir.entry_block]
                .ops
                .iter()
                .all(|op| op.opcode != OpCode::Copy),
            "transport marker must be stripped before TIR op assembly"
        );
    }

    // =======================================================================
    // Test 6: string_to_tir_type coverage
    // =======================================================================
    #[test]
    fn string_type_conversion() {
        assert_eq!(string_to_tir_type("int"), TirType::I64);
        assert_eq!(string_to_tir_type("i64"), TirType::I64);
        assert_eq!(string_to_tir_type("float"), TirType::F64);
        assert_eq!(string_to_tir_type("f64"), TirType::F64);
        assert_eq!(string_to_tir_type("bool"), TirType::Bool);
        assert_eq!(string_to_tir_type("str"), TirType::Str);
        assert_eq!(string_to_tir_type("bytes"), TirType::Bytes);
        assert_eq!(string_to_tir_type("None"), TirType::None);
        assert_eq!(string_to_tir_type("none"), TirType::None);
        assert_eq!(
            string_to_tir_type("list[int]"),
            TirType::List(Box::new(TirType::I64))
        );
        assert_eq!(
            string_to_tir_type("dict[str, float]"),
            TirType::Dict(Box::new(TirType::Str), Box::new(TirType::F64))
        );
        assert_eq!(string_to_tir_type("unknown_type"), TirType::DynBox);
    }
}
