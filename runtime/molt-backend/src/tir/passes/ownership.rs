//! Ownership-specialized function instantiation analysis (Lobster-inspired).
//!
//! Classifies each argument at every call site as:
//! - `Owned`: the value's last use is this call — ownership can be transferred
//!   to the callee, avoiding an IncRef/DecRef pair entirely.
//! - `Borrowed`: the value is used again after the call — the callee must not
//!   store or consume the reference.
//! - `Unknown`: cannot determine statically (e.g. aliased through memory,
//!   used in multiple branches with different liveness).
//!
//! Downstream passes (refcount elimination, callee specialization) consume
//! this information to elide reference-count traffic on owned transfers and
//! to emit borrow-only calling conventions for borrowed arguments.

use std::collections::{HashMap, HashSet};

use crate::tir::blocks::Terminator;
use crate::tir::function::TirFunction;
use crate::tir::ops::OpCode;
use crate::tir::values::ValueId;

/// Ownership classification for a single argument at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    /// Last use of the value — callee can take ownership (no IncRef needed).
    Owned,
    /// Value is used again after this call — callee must borrow only.
    Borrowed,
    /// Cannot determine ownership statically.
    Unknown,
}

/// Key for the ownership map: (call-site result ValueId, argument index).
///
/// The call-site result ValueId uniquely identifies the call within the
/// function (every Call/CallBuiltin/CallMethod produces at least one result).
/// The argument index identifies which operand of the call we're classifying.
pub type CallSiteArgKey = (ValueId, usize);

/// Build a use-set: for each ValueId, collect all (block_index, op_index)
/// positions where it appears as an operand OR in a terminator.
///
/// A value's "use" includes every instruction and terminator that reads it.
/// We track both the block-local op index and the block id so we can
/// determine whether a use is "after" a given call site.
struct UsePosition {
    block: crate::tir::blocks::BlockId,
    /// Index within block.ops. `usize::MAX` means the use is in the terminator.
    op_idx: usize,
}

fn build_use_positions(func: &TirFunction) -> HashMap<ValueId, Vec<UsePosition>> {
    let mut uses: HashMap<ValueId, Vec<UsePosition>> = HashMap::new();

    for (&bid, block) in &func.blocks {
        for (op_idx, op) in block.ops.iter().enumerate() {
            for &operand in &op.operands {
                uses.entry(operand).or_default().push(UsePosition {
                    block: bid,
                    op_idx,
                });
            }
        }

        // Terminator uses.
        let term_vals: Vec<ValueId> = match &block.terminator {
            Terminator::Return { values } => values.clone(),
            Terminator::Branch { args, .. } => args.clone(),
            Terminator::CondBranch {
                cond,
                then_args,
                else_args,
                ..
            } => {
                let mut v = vec![*cond];
                v.extend(then_args);
                v.extend(else_args);
                v
            }
            Terminator::Switch {
                value,
                cases,
                default_args,
                ..
            } => {
                let mut v = vec![*value];
                for (_, _, args) in cases {
                    v.extend(args);
                }
                v.extend(default_args);
                v
            }
            Terminator::Unreachable => vec![],
        };
        for val in term_vals {
            uses.entry(val).or_default().push(UsePosition {
                block: bid,
                op_idx: usize::MAX, // sentinel for terminator
            });
        }
    }

    uses
}

/// Returns true if `val` has any use strictly after position (`call_block`, `call_op_idx`).
///
/// "Strictly after" means:
/// - Same block, higher op index (or in the terminator).
/// - Different block (conservative: if the value is used in any other block,
///   we assume it may be live after the call since we don't do full liveness
///   analysis here -- that would require a fixpoint over the CFG).
fn has_use_after(
    uses: &HashMap<ValueId, Vec<UsePosition>>,
    val: ValueId,
    call_block: crate::tir::blocks::BlockId,
    call_op_idx: usize,
) -> bool {
    let positions = match uses.get(&val) {
        Some(p) => p,
        None => return false,
    };
    for pos in positions {
        if pos.block == call_block {
            if pos.op_idx > call_op_idx {
                return true;
            }
        } else {
            // Used in a different block -- conservatively assume it's live
            // after the call. A full liveness analysis would refine this,
            // but for the initial implementation, correctness > precision.
            return true;
        }
    }
    false
}

/// Analyze call-site argument ownership for all calls in `func`.
///
/// Returns a map from `(call_result_id, arg_index)` to [`Ownership`].
/// Only Call, CallBuiltin, and CallMethod opcodes are analyzed.
pub fn analyze_call_ownership(func: &TirFunction) -> HashMap<CallSiteArgKey, Ownership> {
    let uses = build_use_positions(func);
    let mut result: HashMap<CallSiteArgKey, Ownership> = HashMap::new();

    // Collect all values defined by Alloc/StackAlloc -- these have clear
    // single-owner semantics. Other values (constants, parameters, results
    // of other calls) get `Unknown` unless we can prove last-use.
    let mut alloc_vals: HashSet<ValueId> = HashSet::new();
    for block in func.blocks.values() {
        for op in &block.ops {
            if op.opcode == OpCode::Alloc || op.opcode == OpCode::StackAlloc {
                for &r in &op.results {
                    alloc_vals.insert(r);
                }
            }
        }
    }

    for (&bid, block) in &func.blocks {
        for (op_idx, op) in block.ops.iter().enumerate() {
            let is_call = matches!(
                op.opcode,
                OpCode::Call | OpCode::CallBuiltin | OpCode::CallMethod
            );
            if !is_call {
                continue;
            }

            // Use the first result as the call-site identifier. If no results,
            // use a synthetic key from the operand list hash -- but in practice
            // all calls produce at least one result in TIR.
            let call_id = match op.results.first() {
                Some(&r) => r,
                None => continue,
            };

            for (arg_idx, &arg_val) in op.operands.iter().enumerate() {
                // Unboxed scalars (constants) don't need ownership tracking --
                // they're copied by value. Skip them to reduce noise.
                // We check if the value is ever produced by a ConstInt/ConstFloat/
                // ConstBool/ConstNone/ConstStr/ConstBytes op.
                // For simplicity, we classify all values and let downstream
                // consumers decide what to do with scalar ownership info.

                let ownership = if has_use_after(&uses, arg_val, bid, op_idx) {
                    // Value is used after this call -- callee must borrow.
                    Ownership::Borrowed
                } else if alloc_vals.contains(&arg_val) {
                    // Value is an allocation with no uses after this call --
                    // ownership can be transferred.
                    Ownership::Owned
                } else {
                    // Not an allocation, but last use -- conservatively mark
                    // as Owned since the value won't be accessed again.
                    // This is safe because the caller's reference is the only
                    // one we're tracking, and it dies here.
                    Ownership::Owned
                };

                result.insert((call_id, arg_idx), ownership);
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::Terminator;
    use crate::tir::function::TirFunction;
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_op(opcode: OpCode, operands: Vec<ValueId>, results: Vec<ValueId>) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands,
            results,
            attrs: AttrDict::new(),
            source_span: None,
        }
    }

    /// Test: argument used only at the call site → Owned.
    #[test]
    fn last_use_arg_is_owned() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let call_result = func.fresh_value();
        let const_none = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry.ops.push(make_op(
            OpCode::Call,
            vec![alloc_val],
            vec![call_result],
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
        entry.terminator = Terminator::Return {
            values: vec![const_none],
        };

        let ownership = analyze_call_ownership(&func);
        assert_eq!(
            ownership[&(call_result, 0)],
            Ownership::Owned,
            "alloc_val is last-used at Call → Owned"
        );
    }

    /// Test: argument used after the call → Borrowed.
    #[test]
    fn reused_arg_is_borrowed() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let alloc_val = func.fresh_value();
        let call_result = func.fresh_value();
        let load_result = func.fresh_value();
        let const_none = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry
            .ops
            .push(make_op(OpCode::Alloc, vec![], vec![alloc_val]));
        entry.ops.push(make_op(
            OpCode::Call,
            vec![alloc_val],
            vec![call_result],
        ));
        // alloc_val used again after the call:
        entry.ops.push(make_op(
            OpCode::LoadAttr,
            vec![alloc_val],
            vec![load_result],
        ));
        entry
            .ops
            .push(make_op(OpCode::ConstNone, vec![], vec![const_none]));
        entry.terminator = Terminator::Return {
            values: vec![const_none],
        };

        let ownership = analyze_call_ownership(&func);
        assert_eq!(
            ownership[&(call_result, 0)],
            Ownership::Borrowed,
            "alloc_val is used after Call → Borrowed"
        );
    }

    /// Test: argument used in the return terminator → Borrowed.
    #[test]
    fn arg_in_return_is_borrowed() {
        let mut func = TirFunction::new("f".into(), vec![TirType::DynBox], TirType::DynBox);
        let param = ValueId(0);
        let call_result = func.fresh_value();

        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(make_op(
            OpCode::Call,
            vec![param],
            vec![call_result],
        ));
        // param is also returned → live after the call
        entry.terminator = Terminator::Return {
            values: vec![param],
        };

        let ownership = analyze_call_ownership(&func);
        assert_eq!(
            ownership[&(call_result, 0)],
            Ownership::Borrowed,
            "param returned after Call → Borrowed"
        );
    }

    /// Test: empty function → no ownership entries.
    #[test]
    fn empty_function_no_entries() {
        let func = TirFunction::new("empty".into(), vec![], TirType::None);
        let ownership = analyze_call_ownership(&func);
        assert!(ownership.is_empty());
    }
}
