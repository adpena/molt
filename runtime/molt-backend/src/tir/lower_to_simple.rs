//! TIR → SimpleIR back-conversion scaffold.
//!
//! This module provides the bridge that allows TIR optimization passes to
//! benefit Cranelift and WASM backends without rewriting them.
//!
//! # Phase 1 (current)
//! Basic linearization: visits blocks in reverse-postorder, converts block
//! arguments at join points to `store_var` ops, and maps TIR terminators back
//! to SimpleIR control-flow markers.
//!
//! # Phase 2
//! Full round-trip with type annotations, phi elimination, and all OpCode
//! mappings.

use std::collections::{HashMap, HashSet};

use crate::ir::OpIR;

use super::blocks::{BlockId, Terminator, TirBlock};
use super::function::TirFunction;
use super::ops::{AttrValue, OpCode, TirOp};
use super::values::ValueId;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Convert a [`TirFunction`] back to a linear sequence of [`OpIR`] entries
/// suitable for the existing Cranelift/WASM/Luau backends.
///
/// The conversion linearises blocks in reverse-postorder (entry first, then
/// successors), emitting:
/// - A `label` op at the start of each non-entry block.
/// - `store_var` ops for block arguments at join points.
/// - One [`OpIR`] per [`TirOp`] in the block.
/// - Control-flow [`OpIR`] ops derived from the block's [`Terminator`].
pub fn lower_to_simple_ir(func: &TirFunction) -> Vec<OpIR> {
    let mut out = Vec::new();

    // Compute block visit order (reverse-postorder from entry).
    let rpo = reverse_postorder(func);

    // Collect block argument info for all blocks so we can generate
    // `store_var` assignments at branch sites.
    // Map: (source_block, target_block) → Vec<(arg_value, param_var_name)>
    // We synthesise variable names for block arguments as "_bb<id>_arg<n>".

    // Build param-variable names for every block that has args.
    let block_param_vars: HashMap<BlockId, Vec<String>> = func
        .blocks
        .iter()
        .map(|(bid, block)| {
            let vars: Vec<String> = block
                .args
                .iter()
                .enumerate()
                .map(|(i, _)| format!("_bb{}_arg{}", bid.0, i))
                .collect();
            (*bid, vars)
        })
        .collect();

    // Emit the function prologue: map entry block args → parameter variables.
    if let Some(entry_block) = func.blocks.get(&func.entry_block) {
        for (i, arg) in entry_block.args.iter().enumerate() {
            out.push(OpIR {
                kind: "load_param".to_string(),
                value: Some(i as i64),
                out: Some(value_var(arg.id)),
                ..OpIR::default()
            });
        }
    }

    for bid in &rpo {
        let block = match func.blocks.get(bid) {
            Some(b) => b,
            None => continue,
        };

        // Emit a label for every block except the entry.
        if *bid != func.entry_block {
            out.push(OpIR {
                kind: "label".to_string(),
                value: Some(bid.0 as i64),
                ..OpIR::default()
            });

            // Load block argument variables into SSA-named vars.
            if let Some(param_vars) = block_param_vars.get(bid) {
                for (i, var_name) in param_vars.iter().enumerate() {
                    if i < block.args.len() {
                        // The actual value was stored into `var_name` by
                        // the predecessor's terminator emission. Load it.
                        out.push(OpIR {
                            kind: "load_var".to_string(),
                            var: Some(var_name.clone()),
                            out: Some(value_var(block.args[i].id)),
                            ..OpIR::default()
                        });
                    }
                }
            }
        }

        // Emit ops.
        for op in &block.ops {
            if let Some(opir) = lower_op(op) {
                out.push(opir);
            }
        }

        // Emit terminator.
        emit_terminator(block, &block_param_vars, &mut out);
    }

    out
}

// ---------------------------------------------------------------------------
// Op lowering
// ---------------------------------------------------------------------------

/// Convert a single TirOp to an OpIR. Returns None for ops that are
/// dialect-internal and have no SimpleIR equivalent (yet).
fn lower_op(op: &TirOp) -> Option<OpIR> {
    // Map result (if any) to output variable.
    let out_var = op.results.first().map(|v| value_var(*v));

    match op.opcode {
        // Constants.
        OpCode::ConstInt => Some(OpIR {
            kind: "const".to_string(),
            value: attr_int(&op.attrs, "value"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstFloat => Some(OpIR {
            kind: "const".to_string(),
            f_value: attr_float(&op.attrs, "value"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstStr => Some(OpIR {
            kind: "const".to_string(),
            s_value: attr_str(&op.attrs, "value"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstBool => Some(OpIR {
            kind: "const".to_string(),
            value: Some(if attr_bool(&op.attrs, "value").unwrap_or(false) { 1 } else { 0 }),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstNone => Some(OpIR {
            kind: "const_none".to_string(),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ConstBytes => Some(OpIR {
            kind: "const".to_string(),
            bytes: attr_bytes(&op.attrs, "value"),
            out: out_var,
            ..OpIR::default()
        }),

        // Arithmetic.
        OpCode::Add => Some(binary_op("add", op, out_var)),
        OpCode::Sub => Some(binary_op("sub", op, out_var)),
        OpCode::Mul => Some(binary_op("mul", op, out_var)),
        OpCode::Div => Some(binary_op("div", op, out_var)),
        OpCode::FloorDiv => Some(binary_op("floor_div", op, out_var)),
        OpCode::Mod => Some(binary_op("mod", op, out_var)),
        OpCode::Pow => Some(binary_op("pow", op, out_var)),
        OpCode::Neg => Some(unary_op("neg", op, out_var)),
        OpCode::Pos => Some(unary_op("pos", op, out_var)),

        // Comparison.
        OpCode::Eq => Some(binary_op("eq", op, out_var)),
        OpCode::Ne => Some(binary_op("ne", op, out_var)),
        OpCode::Lt => Some(binary_op("lt", op, out_var)),
        OpCode::Le => Some(binary_op("le", op, out_var)),
        OpCode::Gt => Some(binary_op("gt", op, out_var)),
        OpCode::Ge => Some(binary_op("ge", op, out_var)),
        OpCode::Is => Some(binary_op("is", op, out_var)),
        OpCode::IsNot => Some(binary_op("is_not", op, out_var)),
        OpCode::In => Some(binary_op("in", op, out_var)),
        OpCode::NotIn => Some(binary_op("not_in", op, out_var)),

        // Bitwise.
        OpCode::BitAnd => Some(binary_op("bit_and", op, out_var)),
        OpCode::BitOr => Some(binary_op("bit_or", op, out_var)),
        OpCode::BitXor => Some(binary_op("bit_xor", op, out_var)),
        OpCode::BitNot => Some(unary_op("bit_not", op, out_var)),
        OpCode::Shl => Some(binary_op("shl", op, out_var)),
        OpCode::Shr => Some(binary_op("shr", op, out_var)),

        // Boolean.
        OpCode::And => Some(binary_op("and", op, out_var)),
        OpCode::Or => Some(binary_op("or", op, out_var)),
        OpCode::Not => Some(unary_op("not", op, out_var)),

        // Memory.
        OpCode::LoadAttr => Some(OpIR {
            kind: "get_attr".to_string(),
            args: Some(operand_args(op)),
            s_value: attr_str(&op.attrs, "name"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::StoreAttr => Some(OpIR {
            kind: "set_attr".to_string(),
            args: Some(operand_args(op)),
            s_value: attr_str(&op.attrs, "name"),
            ..OpIR::default()
        }),
        OpCode::Index => Some(binary_op("subscript", op, out_var)),
        OpCode::StoreIndex => Some(OpIR {
            kind: "store_subscript".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Call.
        OpCode::Call => Some(OpIR {
            kind: "call".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::CallMethod => Some(OpIR {
            kind: "call_method".to_string(),
            args: Some(operand_args(op)),
            s_value: attr_str(&op.attrs, "method"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::CallBuiltin => Some(OpIR {
            kind: "call_builtin".to_string(),
            args: Some(operand_args(op)),
            s_value: attr_str(&op.attrs, "name"),
            out: out_var,
            ..OpIR::default()
        }),

        // Box/unbox — no-ops at SimpleIR level (type info discarded).
        OpCode::BoxVal | OpCode::UnboxVal | OpCode::TypeGuard | OpCode::Copy => {
            if let (Some(src), Some(dst)) = (op.operands.first(), op.results.first()) {
                Some(OpIR {
                    kind: "copy_var".to_string(),
                    var: Some(value_var(*src)),
                    out: Some(value_var(*dst)),
                    ..OpIR::default()
                })
            } else {
                None
            }
        }

        // Build containers.
        OpCode::BuildList => Some(OpIR {
            kind: "build_list".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildDict => Some(OpIR {
            kind: "build_dict".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildTuple => Some(OpIR {
            kind: "build_tuple".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildSet => Some(OpIR {
            kind: "build_set".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::BuildSlice => Some(OpIR {
            kind: "build_slice".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Iteration.
        OpCode::GetIter => Some(unary_op("get_iter", op, out_var)),
        OpCode::IterNext => Some(unary_op("iter_next", op, out_var)),
        OpCode::ForIter => Some(OpIR {
            kind: "for_iter".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Generator.
        OpCode::Yield => Some(OpIR {
            kind: "yield".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::YieldFrom => Some(OpIR {
            kind: "yield_from".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Exception.
        OpCode::Raise => Some(OpIR {
            kind: "raise".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
        OpCode::CheckException => Some(OpIR {
            kind: "check_exception".to_string(),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Import.
        OpCode::Import => Some(OpIR {
            kind: "import".to_string(),
            s_value: attr_str(&op.attrs, "module"),
            out: out_var,
            ..OpIR::default()
        }),
        OpCode::ImportFrom => Some(OpIR {
            kind: "import_from".to_string(),
            s_value: attr_str(&op.attrs, "name"),
            args: Some(operand_args(op)),
            out: out_var,
            ..OpIR::default()
        }),

        // Refcount — no-ops at SimpleIR level.
        OpCode::IncRef | OpCode::DecRef | OpCode::Alloc | OpCode::StackAlloc | OpCode::Free => {
            None
        }

        // SCF ops — handled separately via terminators in Phase 2.
        OpCode::ScfIf | OpCode::ScfFor | OpCode::ScfWhile | OpCode::ScfYield => None,

        // Deopt — emit a hint but not critical.
        OpCode::Deopt => Some(OpIR {
            kind: "deopt".to_string(),
            ..OpIR::default()
        }),

        // Remaining attribute ops.
        OpCode::DelAttr => Some(OpIR {
            kind: "del_attr".to_string(),
            args: Some(operand_args(op)),
            s_value: attr_str(&op.attrs, "name"),
            ..OpIR::default()
        }),
        OpCode::DelIndex => Some(OpIR {
            kind: "del_subscript".to_string(),
            args: Some(operand_args(op)),
            ..OpIR::default()
        }),
    }
}

// ---------------------------------------------------------------------------
// Terminator emission
// ---------------------------------------------------------------------------

fn emit_terminator(
    block: &TirBlock,
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    out: &mut Vec<OpIR>,
) {
    match &block.terminator {
        Terminator::Return { values } => {
            if values.is_empty() {
                out.push(OpIR {
                    kind: "ret_void".to_string(),
                    ..OpIR::default()
                });
            } else {
                out.push(OpIR {
                    kind: "ret".to_string(),
                    args: Some(values.iter().map(|v| value_var(*v)).collect()),
                    ..OpIR::default()
                });
            }
        }

        Terminator::Branch { target, args } => {
            // Store args into target block's parameter variables.
            emit_block_arg_stores(*target, args, block_param_vars, out);
            out.push(OpIR {
                kind: "jump".to_string(),
                value: Some(target.0 as i64),
                ..OpIR::default()
            });
        }

        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            // Emit: br_if cond → then_block; else → else_block
            // We use SimpleIR `if`/`else`/`end_if` for structured representation,
            // or `br_if` + `jump` for unstructured. Use `br_if` here since we
            // don't have structural nesting info.
            out.push(OpIR {
                kind: "br_if".to_string(),
                args: Some(vec![value_var(*cond)]),
                value: Some(then_block.0 as i64),
                ..OpIR::default()
            });
            // Else path: store else_args and jump to else_block.
            emit_block_arg_stores(*else_block, else_args, block_param_vars, out);
            out.push(OpIR {
                kind: "jump".to_string(),
                value: Some(else_block.0 as i64),
                ..OpIR::default()
            });
            // Then landing: label + then_args stores are emitted at the start of
            // then_block when it is visited in RPO.
            // We must still store then_args. Since br_if falls through to the
            // then-block label, store them right before the br_if.
            // But we've already emitted br_if, so we insert before it.
            // Simpler: emit then-arg stores before the br_if.
            // Re-emit in correct order by patching: drain last 2 ops and
            // re-emit with stores first.
            let jump_op = out.pop().unwrap(); // jump to else
            let brif_op = out.pop().unwrap(); // br_if

            // Store then-args before the conditional branch.
            emit_block_arg_stores(*then_block, then_args, block_param_vars, out);

            out.push(brif_op);
            // After the conditional takes the then path, else path follows:
            emit_block_arg_stores(*else_block, else_args, block_param_vars, out);
            out.push(jump_op);
        }

        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => {
            // Emit a chain of br_if checks for each case, then jump to default.
            for (case_val, target, case_args) in cases {
                // Synthesize a temporary result for the comparison — this is a
                // Phase 1 approximation; Phase 2 will use proper SSA values.
                out.push(OpIR {
                    kind: "switch_case".to_string(),
                    args: Some(vec![value_var(*value)]),
                    value: Some(*case_val),
                    ..OpIR::default()
                });
                emit_block_arg_stores(*target, case_args, block_param_vars, out);
                out.push(OpIR {
                    kind: "jump".to_string(),
                    value: Some(target.0 as i64),
                    ..OpIR::default()
                });
            }
            emit_block_arg_stores(*default, default_args, block_param_vars, out);
            out.push(OpIR {
                kind: "jump".to_string(),
                value: Some(default.0 as i64),
                ..OpIR::default()
            });
        }

        Terminator::Unreachable => {
            out.push(OpIR {
                kind: "unreachable".to_string(),
                ..OpIR::default()
            });
        }
    }
}

/// Emit `store_var` ops to pass `values` to the target block's arg variables.
fn emit_block_arg_stores(
    target: BlockId,
    values: &[ValueId],
    block_param_vars: &HashMap<BlockId, Vec<String>>,
    out: &mut Vec<OpIR>,
) {
    if values.is_empty() {
        return;
    }
    if let Some(param_vars) = block_param_vars.get(&target) {
        for (i, val) in values.iter().enumerate() {
            if let Some(var_name) = param_vars.get(i) {
                out.push(OpIR {
                    kind: "store_var".to_string(),
                    var: Some(var_name.clone()),
                    args: Some(vec![value_var(*val)]),
                    ..OpIR::default()
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RPO traversal
// ---------------------------------------------------------------------------

fn reverse_postorder(func: &TirFunction) -> Vec<BlockId> {
    let mut visited: HashSet<BlockId> = HashSet::new();
    let mut postorder: Vec<BlockId> = Vec::new();
    let mut stack: Vec<(BlockId, bool)> = vec![(func.entry_block, false)];

    while let Some((bid, processed)) = stack.pop() {
        if processed {
            postorder.push(bid);
            continue;
        }
        if visited.contains(&bid) {
            continue;
        }
        visited.insert(bid);
        stack.push((bid, true));

        if let Some(block) = func.blocks.get(&bid) {
            // Push successors in reverse order for correct DFS.
            let succs = successors_of(block);
            for succ in succs.into_iter().rev() {
                if !visited.contains(&succ) {
                    stack.push((succ, false));
                }
            }
        }
    }

    postorder.reverse();
    postorder
}

fn successors_of(block: &TirBlock) -> Vec<BlockId> {
    match &block.terminator {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut succs = vec![*default];
            for (_, target, _) in cases {
                succs.push(*target);
            }
            succs
        }
        Terminator::Return { .. } | Terminator::Unreachable => vec![],
    }
}

// ---------------------------------------------------------------------------
// Helper utilities
// ---------------------------------------------------------------------------

/// Synthesise a SimpleIR variable name from a ValueId.
fn value_var(id: ValueId) -> String {
    format!("_v{}", id.0)
}

fn binary_op(kind: &str, op: &TirOp, out: Option<String>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(operand_args(op)),
        out,
        ..OpIR::default()
    }
}

fn unary_op(kind: &str, op: &TirOp, out: Option<String>) -> OpIR {
    OpIR {
        kind: kind.to_string(),
        args: Some(operand_args(op)),
        out,
        ..OpIR::default()
    }
}

fn operand_args(op: &TirOp) -> Vec<String> {
    op.operands.iter().map(|v| value_var(*v)).collect()
}

fn attr_int(attrs: &super::ops::AttrDict, key: &str) -> Option<i64> {
    match attrs.get(key) {
        Some(AttrValue::Int(i)) => Some(*i),
        _ => None,
    }
}

fn attr_float(attrs: &super::ops::AttrDict, key: &str) -> Option<f64> {
    match attrs.get(key) {
        Some(AttrValue::Float(f)) => Some(*f),
        _ => None,
    }
}

fn attr_str(attrs: &super::ops::AttrDict, key: &str) -> Option<String> {
    match attrs.get(key) {
        Some(AttrValue::Str(s)) => Some(s.clone()),
        _ => None,
    }
}

fn attr_bool(attrs: &super::ops::AttrDict, key: &str) -> Option<bool> {
    match attrs.get(key) {
        Some(AttrValue::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn attr_bytes(attrs: &super::ops::AttrDict, key: &str) -> Option<Vec<u8>> {
    match attrs.get(key) {
        Some(AttrValue::Bytes(b)) => Some(b.clone()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{Terminator, TirBlock};
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn add_function() -> TirFunction {
        let mut func = TirFunction::new(
            "add".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
        );

        let result = ValueId(func.next_value);
        func.next_value += 1;

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![result],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return {
            values: vec![result],
        };

        func
    }

    #[test]
    fn linearize_simple_function_compiles() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func);
        // Must produce at least one op.
        assert!(!ops.is_empty(), "expected non-empty ops for add function");
    }

    #[test]
    fn linearize_emits_return() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func);
        let has_ret = ops.iter().any(|o| o.kind == "ret" || o.kind == "ret_void");
        assert!(has_ret, "expected a return op, got: {:?}", ops);
    }

    #[test]
    fn linearize_emits_add_op() {
        let func = add_function();
        let ops = lower_to_simple_ir(&func);
        let has_add = ops.iter().any(|o| o.kind == "add");
        assert!(has_add, "expected an 'add' op, got: {:?}", ops);
    }

    #[test]
    fn linearize_multi_block_emits_labels() {
        // Build: func @branch(bool) -> i64 with two successor blocks.
        let mut func = TirFunction::new("branch".into(), vec![TirType::Bool], TirType::I64);

        let bb1 = func.fresh_block();
        let bb2 = func.fresh_block();
        let v1 = func.fresh_value();
        let v2 = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: bb1,
            then_args: vec![],
            else_block: bb2,
            else_args: vec![],
        };

        let mut attrs1 = AttrDict::new();
        attrs1.insert("value".into(), AttrValue::Int(1));
        func.blocks.insert(
            bb1,
            TirBlock {
                id: bb1,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v1],
                    attrs: attrs1,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v1] },
            },
        );

        let mut attrs2 = AttrDict::new();
        attrs2.insert("value".into(), AttrValue::Int(0));
        func.blocks.insert(
            bb2,
            TirBlock {
                id: bb2,
                args: vec![],
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v2],
                    attrs: attrs2,
                    source_span: None,
                }],
                terminator: Terminator::Return { values: vec![v2] },
            },
        );

        let ops = lower_to_simple_ir(&func);
        let label_count = ops.iter().filter(|o| o.kind == "label").count();
        // Should have labels for bb1 and bb2.
        assert!(
            label_count >= 2,
            "expected >=2 labels for multi-block function, got {}: {:?}",
            label_count,
            ops
        );
    }

    #[test]
    fn value_var_naming() {
        assert_eq!(value_var(ValueId(0)), "_v0");
        assert_eq!(value_var(ValueId(42)), "_v42");
    }
}
