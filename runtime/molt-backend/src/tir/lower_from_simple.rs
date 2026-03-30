//! SimpleIR → TIR construction pipeline.
//!
//! Chains together CFG extraction, SSA conversion, and TIR function assembly
//! into a single `lower_to_tir` entry point.

use std::any::Any;
use std::collections::HashMap;

use crate::ir::FunctionIR;

use super::blocks::{BlockId, LoopRole, TirBlock};
use super::cfg::CFG;
use super::function::TirFunction;
use super::ssa::{SsaOutput, convert_to_ssa_with_params};
use super::types::TirType;
use super::values::ValueId;

/// Convert a SimpleIR function into a fully-constructed TIR function.
///
/// Pipeline: SimpleIR ops → CFG extraction → SSA conversion → TIR construction.
///
/// Type hints from the SimpleIR metadata (`fast_int`, `fast_float`, `type_hint`)
/// are propagated as initial seed types on the SSA values that correspond to
/// ops carrying those hints. All other values start as `DynBox`.
pub fn lower_to_tir(ir: &FunctionIR) -> TirFunction {
    // 1. Build CFG from the linear op stream.
    let cfg = CFG::build(&ir.ops);

    // 2. Convert to SSA with block arguments (pass params for implicit entry defs).
    let ssa = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        convert_to_ssa_with_params(&cfg, &ir.ops, &ir.params)
    }))
    .unwrap_or_else(|payload| {
        panic!(
            "SSA conversion failed for '{}': {}",
            ir.name,
            panic_payload_message(payload.as_ref())
        )
    });

    // 3. Assemble the TirFunction from the SSA output.
    assemble_function(ir, &cfg, ssa)
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(msg) = payload.downcast_ref::<String>() {
        return msg.clone();
    }
    if let Some(msg) = payload.downcast_ref::<&'static str>() {
        return (*msg).to_string();
    }
    "non-string panic payload".to_string()
}

/// Assemble a `TirFunction` from a `FunctionIR`, its `CFG`, and the `SsaOutput`.
fn assemble_function(ir: &FunctionIR, cfg: &CFG, ssa: SsaOutput) -> TirFunction {
    let SsaOutput {
        blocks: tir_blocks,
        mut types,
        next_value,
    } = ssa;

    // Apply initial type hints from fast_int / fast_float / type_hint metadata.
    apply_type_hints(ir, cfg, &tir_blocks, &mut types);

    // Determine parameter types — default to DynBox, but honour param_types if
    // the frontend provided string annotations.
    let param_types: Vec<TirType> = if let Some(ref pt) = ir.param_types {
        pt.iter().map(|s| string_to_tir_type(s)).collect()
    } else {
        ir.params.iter().map(|_| TirType::DynBox).collect()
    };

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
        block.ops.iter().any(|op| {
            matches!(
                op.opcode,
                super::ops::OpCode::TryStart
                    | super::ops::OpCode::TryEnd
                    | super::ops::OpCode::StateBlockStart
                    | super::ops::OpCode::StateBlockEnd
                    | super::ops::OpCode::CheckException
            )
        })
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
    let loop_roles = detect_loop_roles(ir, cfg, &block_map);

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
                a.insert("_original_has_ret".into(), super::ops::AttrValue::Bool(true));
            }
            a
        },
        has_exception_handling,
        label_id_map,
        loop_roles,
    }
}

/// Scan the original SimpleIR ops and CFG to detect which TIR blocks correspond
/// to `loop_start` and `loop_end` structural markers.  Returns a map from
/// BlockId to LoopRole for every block that has a loop-structural role.
fn detect_loop_roles(
    ir: &FunctionIR,
    cfg: &CFG,
    _block_map: &HashMap<BlockId, TirBlock>,
) -> HashMap<BlockId, LoopRole> {
    let mut roles = HashMap::new();
    for (bid, bb) in cfg.blocks.iter().enumerate() {
        if bb.start_op >= ir.ops.len() {
            continue;
        }
        let first_kind = ir.ops[bb.start_op].kind.as_str();
        match first_kind {
            "loop_start" => {
                roles.insert(BlockId(bid as u32), LoopRole::LoopHeader);
            }
            "loop_end" => {
                roles.insert(BlockId(bid as u32), LoopRole::LoopEnd);
            }
            _ => {}
        }
    }
    roles
}

/// Walk the original ops and propagate `fast_int` / `fast_float` / `type_hint`
/// metadata to the corresponding SSA result values.
///
/// The SSA pass creates result ValueIds sequentially as it visits ops. We
/// replay the same visitation order (skipping structural ops) to correlate
/// each SimpleIR op with its TIR result(s).
fn apply_type_hints(
    ir: &FunctionIR,
    cfg: &CFG,
    tir_blocks: &[TirBlock],
    types: &mut HashMap<ValueId, TirType>,
) {
    // For each TIR block, walk its ops and match against the original OpIR
    // to propagate type hints.
    for (bid, tir_block) in tir_blocks.iter().enumerate() {
        // Each TIR op's results correspond to an op from the original stream.
        // We can correlate via the CFG block's op range and the SSA's skip
        // of structural ops. The TIR ops in each block correspond 1:1 to the
        // non-structural ops in that CFG block.
        if bid >= cfg.blocks.len() {
            continue;
        }
        let bb = &cfg.blocks[bid];

        // Collect non-structural op indices from the CFG block (same logic as SSA).
        let mut non_structural_idx = 0;
        for op_idx in bb.start_op..bb.end_op {
            if is_structural(&ir.ops[op_idx].kind) {
                continue;
            }

            // Match this to the tir_block op at the same position.
            if non_structural_idx >= tir_block.ops.len() {
                break;
            }
            let tir_op = &tir_block.ops[non_structural_idx];
            let src_op = &ir.ops[op_idx];

            // Apply type hints to each result of this TIR op.
            if let Some(hint_type) = resolve_type_hint(src_op) {
                for &result_vid in &tir_op.results {
                    types.insert(result_vid, hint_type.clone());
                }
            }

            non_structural_idx += 1;
        }
    }
}

/// Determine a type hint from a SimpleIR op's metadata fields.
fn resolve_type_hint(op: &crate::ir::OpIR) -> Option<TirType> {
    // Explicit type_hint string takes priority.
    if let Some(ref hint) = op.type_hint {
        let ty = string_to_tir_type(hint);
        if ty != TirType::DynBox {
            return Some(ty);
        }
    }
    // fast_int / fast_float flags.
    if op.fast_int == Some(true) {
        return Some(TirType::I64);
    }
    if op.fast_float == Some(true) {
        return Some(TirType::F64);
    }
    None
}

/// Convert a string type annotation to a `TirType`.
fn string_to_tir_type(s: &str) -> TirType {
    match s {
        "int" | "i64" => TirType::I64,
        "float" | "f64" => TirType::F64,
        "bool" => TirType::Bool,
        "str" => TirType::Str,
        "bytes" => TirType::Bytes,
        "None" | "none" => TirType::None,
        "list" => TirType::List(Box::new(TirType::DynBox)),
        "dict" => TirType::Dict(Box::new(TirType::DynBox), Box::new(TirType::DynBox)),
        "set" => TirType::Set(Box::new(TirType::DynBox)),
        "tuple" => TirType::Tuple(vec![]),
        _ => TirType::DynBox,
    }
}

/// Infer the function return type by examining all Return terminators.
/// Uses a lattice meet to combine multiple return types.
fn infer_return_type(blocks: &[TirBlock], types: &HashMap<ValueId, TirType>) -> TirType {
    use super::blocks::Terminator;

    let mut result_type: Option<TirType> = None;

    for block in blocks {
        if let Terminator::Return { values } = &block.terminator {
            let ret_ty = if values.is_empty() {
                TirType::None
            } else {
                // Use the type of the first return value.
                values
                    .first()
                    .and_then(|vid| types.get(vid))
                    .cloned()
                    .unwrap_or(TirType::DynBox)
            };

            result_type = Some(match result_type {
                None => ret_ty,
                Some(existing) => existing.meet(&ret_ty),
            });
        }
    }

    result_type.unwrap_or(TirType::None)
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
    use crate::tir::types::TirType;

    /// Helper: build a FunctionIR with given name, params, and ops.
    fn make_func(name: &str, params: &[&str], ops: Vec<OpIR>) -> FunctionIR {
        FunctionIR {
            name: name.to_string(),
            params: params.iter().map(|s| s.to_string()).collect(),
            ops,
            param_types: None,
        }
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

    /// Helper: create an op with fast_int hint.
    fn op_fast_int(kind: &str, args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            out: Some(out.to_string()),
            fast_int: Some(true),
            ..OpIR::default()
        }
    }

    /// Helper: create an op with fast_float hint.
    fn op_fast_float(kind: &str, args: &[&str], out: &str) -> OpIR {
        OpIR {
            kind: kind.to_string(),
            args: Some(args.iter().map(|s| s.to_string()).collect()),
            out: Some(out.to_string()),
            fast_float: Some(true),
            ..OpIR::default()
        }
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
        assert_eq!(entry.ops.len(), 2, "entry should have const and add ops");

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

    // =======================================================================
    // Test 3: fast_int propagation
    // =======================================================================
    #[test]
    fn fast_int_type_propagation() {
        let func_ir = make_func(
            "int_add",
            &[],
            vec![
                op_val_out("const", 1, "a"),
                op_val_out("const", 2, "b"),
                op_fast_int("add", &["a", "b"], "c"),
                op_args("ret", &["c"]),
            ],
        );

        let tir = lower_to_tir(&func_ir);

        // Find the add op's result and check its type is I64.
        let entry = &tir.blocks[&tir.entry_block];
        // The add op is the third op (index 2).
        assert!(entry.ops.len() >= 3, "should have at least 3 ops");
        let add_op = &entry.ops[2];
        assert!(!add_op.results.is_empty(), "add op should have a result");
        // The result's type in the function should be I64 because fast_int was set.
        // We don't store types on TirFunction directly, but we can verify via
        // the return type inference — since the only return is `c` which is I64,
        // the return type should be I64.
        assert_eq!(
            tir.return_type,
            TirType::I64,
            "return type should be I64 from fast_int propagation"
        );
    }

    // =======================================================================
    // Test 4: fast_float propagation
    // =======================================================================
    #[test]
    fn fast_float_type_propagation() {
        let func_ir = make_func(
            "float_add",
            &[],
            vec![
                op_val_out("const", 1, "a"),
                op_val_out("const", 2, "b"),
                op_fast_float("add", &["a", "b"], "c"),
                op_args("ret", &["c"]),
            ],
        );

        let tir = lower_to_tir(&func_ir);

        assert_eq!(
            tir.return_type,
            TirType::F64,
            "return type should be F64 from fast_float propagation"
        );
    }

    // =======================================================================
    // Test 5: Empty function
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
    // Test 6: Function with param_types annotation
    // =======================================================================
    #[test]
    fn param_types_from_annotation() {
        let func_ir = FunctionIR {
            name: "typed_add".to_string(),
            params: vec!["a".to_string(), "b".to_string()],
            ops: vec![op_args_out("add", &["a", "b"], "c"), op_args("ret", &["c"])],
            param_types: Some(vec!["int".to_string(), "float".to_string()]),
        };

        let tir = lower_to_tir(&func_ir);

        assert_eq!(tir.param_types.len(), 2);
        assert_eq!(tir.param_types[0], TirType::I64);
        assert_eq!(tir.param_types[1], TirType::F64);
    }

    // =======================================================================
    // Test 7: string_to_tir_type coverage
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
        assert_eq!(string_to_tir_type("unknown_type"), TirType::DynBox);
    }
}
