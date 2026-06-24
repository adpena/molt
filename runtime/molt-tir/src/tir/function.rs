use std::collections::HashMap;

use super::blocks::{BlockId, LoopBreakKind, LoopRole, TirBlock};
use super::ops::AttrDict;
use super::types::TirType;
use super::values::ValueId;

/// A function in TIR: a collection of basic blocks in SSA form.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TirFunction {
    pub name: String,
    /// Original parameter names, aligned 1:1 with `param_types` and the entry
    /// block arguments. These are preserved through the TIR round-trip so
    /// backends do not have to recover parameter identity from synthetic
    /// `load_param` temporaries.
    pub param_names: Vec<String>,
    /// Parameter types (mapped 1:1 to entry block arguments).
    pub param_types: Vec<TirType>,
    /// Return type of this function.
    pub return_type: TirType,
    /// All basic blocks, keyed by BlockId.
    pub blocks: HashMap<BlockId, TirBlock>,
    /// The entry block of this function.
    pub entry_block: BlockId,
    /// Counter for allocating fresh ValueIds.
    pub next_value: u32,
    /// Counter for allocating fresh BlockIds.
    pub next_block: u32,
    /// Function-level attributes (e.g. "fast_math", "closure_specialized").
    pub attrs: AttrDict,
    /// Canonical refined type facts for SSA values that are not block
    /// arguments. Block arguments still carry their type on `TirValue`; this
    /// map mirrors those entries so passes have one function-owned query
    /// surface for every `ValueId`.
    pub value_types: HashMap<ValueId, TirType>,
    /// Set to `true` during lift when the function contains TryStart/TryEnd
    /// or StateBlockStart/StateBlockEnd ops.  When true, aggressive
    /// optimization passes (DCE, SCCP, type refinement, type guard hoist)
    /// must be conservative around exception regions to preserve correctness.
    pub has_exception_handling: bool,
    /// Mapping from TIR BlockId.0 → original SimpleIR label value.
    /// Populated during forward conversion (SimpleIR → TIR) so the
    /// back-conversion can emit labels with the original IDs that ops like
    /// `check_exception`, `jump`, and `br_if` reference via `state_blocks`.
    pub label_id_map: std::collections::HashMap<u32, i64>,
    /// Structural loop roles for blocks — records which blocks are loop
    /// headers (`loop_start`) or loop ends (`loop_end`) so the back-conversion
    /// can re-emit these markers for downstream backends (Cranelift, WASM).
    pub loop_roles: HashMap<BlockId, LoopRole>,
    /// Mapping from loop header block -> matching loop-end block from the
    /// original structured SimpleIR.
    pub loop_pairs: HashMap<BlockId, BlockId>,
    /// Mapping from loop header block -> original loop-break polarity.
    pub loop_break_kinds: HashMap<BlockId, LoopBreakKind>,
    /// Mapping from loop header block -> CFG block that owns the original
    /// `loop_break_if_*` test in SimpleIR. This lets later lowering reuse the
    /// real loop condition block instead of rediscovering it heuristically.
    pub loop_cond_blocks: HashMap<BlockId, BlockId>,
}

impl TirFunction {
    /// Create a new function with a single empty entry block.
    pub fn new(name: String, param_types: Vec<TirType>, return_type: TirType) -> Self {
        use super::blocks::Terminator;
        use super::values::TirValue;

        let entry_id = BlockId(0);
        let mut next_value = 0u32;

        // Create block arguments for the entry block matching param types.
        let args: Vec<TirValue> = param_types
            .iter()
            .map(|ty| {
                let id = ValueId(next_value);
                next_value += 1;
                TirValue { id, ty: ty.clone() }
            })
            .collect();

        let entry = TirBlock {
            id: entry_id,
            args,
            ops: Vec::new(),
            terminator: Terminator::Unreachable,
        };

        let mut value_types = HashMap::new();
        for arg in &entry.args {
            value_types.insert(arg.id, arg.ty.clone());
        }

        let mut blocks = HashMap::new();
        blocks.insert(entry_id, entry);

        Self {
            name,
            param_names: param_types
                .iter()
                .enumerate()
                .map(|(idx, _)| format!("p{idx}"))
                .collect(),
            param_types,
            return_type,
            blocks,
            entry_block: entry_id,
            next_value,
            next_block: 1,
            attrs: AttrDict::new(),
            value_types,
            has_exception_handling: false,
            label_id_map: HashMap::new(),
            loop_roles: HashMap::new(),
            loop_pairs: HashMap::new(),
            loop_break_kinds: HashMap::new(),
            loop_cond_blocks: HashMap::new(),
        }
    }

    /// Allocate a fresh ValueId.
    pub fn fresh_value(&mut self) -> ValueId {
        let id = ValueId(self.next_value);
        self.next_value += 1;
        id
    }

    /// Allocate a fresh BlockId.
    pub fn fresh_block(&mut self) -> BlockId {
        let id = BlockId(self.next_block);
        self.next_block += 1;
        id
    }

    /// True iff the function contains real exception **handler** regions —
    /// `TryStart`/`TryEnd` (a `try`/`except`) or `StateBlockStart`/
    /// `StateBlockEnd` (a generator/async state region).
    ///
    /// This is deliberately narrower than the [`has_exception_handling`] flag,
    /// which is *also* set by [`CheckException`](super::ops::OpCode::CheckException)
    /// observation ops. A `CheckException` in a function with no handler merely
    /// propagates a pending exception to the function's exception EXIT (a
    /// return-with-pending); it is not a handler. After the frontend's universal
    /// exception-observation change (`430e09793`) virtually every real function
    /// carries `CheckException`, so `has_exception_handling` is almost always
    /// true — too coarse for passes whose only hazard is a true handler region.
    ///
    /// Passes that are unsafe **only** around handler regions (the TIR inliner:
    /// splicing across a `try` boundary needs handler-label remapping that this
    /// arc does not perform) gate on this predicate. Passes that must stay
    /// conservative around *any* exception edge (e.g. the augmented-CFG
    /// predecessor consumers) keep using `has_exception_handling`.
    ///
    /// [`has_exception_handling`]: TirFunction::has_exception_handling
    pub fn has_exception_handlers(&self) -> bool {
        use super::ops::OpCode;
        self.blocks.values().any(|block| {
            block.ops.iter().any(|op| {
                matches!(
                    op.opcode,
                    OpCode::TryStart
                        | OpCode::TryEnd
                        | OpCode::StateBlockStart
                        | OpCode::StateBlockEnd
                )
            })
        })
    }

    /// True iff the function is a lowered coroutine `_poll` **state machine** —
    /// it dispatches on a saved state via [`StateSwitch`](super::ops::OpCode::StateSwitch)
    /// (and friends: `StateTransition`/`StateYield`/`ChanSendYield`/
    /// `ChanRecvYield`/`AllocTask`). Such a function's CFG is NOT
    /// dominator-structured: the state dispatch RE-ENTERS resume blocks, so a
    /// value defined in one state region is reachable (via the dispatch back-edge)
    /// in a resume block that a straight-line / dominator liveness walk does NOT
    /// see as dominated. Any pass that places ops keyed on single-entry dominance
    /// (drop insertion's per-block last-use / edge-dying placement) is UNSOUND
    /// over this shape — it can emit a `DecRef` in a resume block referencing a
    /// value defined only on the non-taken first-entry path (a use-before-def that
    /// the LLVM verifier rejects and that double-frees at runtime on native).
    ///
    /// This is the post-lowering complement to [`has_exception_handlers`]: a
    /// generator may be lowered to a `_poll` body carrying `StateSwitch` WITHOUT
    /// the `StateBlockStart`/`StateBlockEnd` delimiters, so the handler predicate
    /// alone does not catch it. Passes whose hazard is the re-entrant state CFG
    /// (drop insertion — design 20 §2.9 handles the high-level suspension model
    /// but NOT the lowered state machine) bail on this predicate too.
    ///
    /// [`has_exception_handlers`]: TirFunction::has_exception_handlers
    pub fn has_state_machine(&self) -> bool {
        use super::blocks::Terminator;
        use super::ops::OpCode;
        self.blocks.values().any(|block| {
            // The `state_switch` dispatch is now a first-class `StateDispatch`
            // terminator (not a body op), so detect it there; the suspend ops
            // (`StateYield`/`StateTransition`/`Chan*Yield`) remain body ops.
            matches!(block.terminator, Terminator::StateDispatch { .. })
                || block.ops.iter().any(|op| {
                    matches!(
                        op.opcode,
                        OpCode::StateSwitch
                            | OpCode::StateTransition
                            | OpCode::StateYield
                            | OpCode::ChanSendYield
                            | OpCode::ChanRecvYield
                            | OpCode::AllocTask
                    )
                })
        })
    }
}

/// A module: a collection of TIR functions.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TirModule {
    pub name: String,
    pub functions: Vec<TirFunction>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, Terminator};
    use crate::tir::ops::{AttrDict, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    #[test]
    fn function_new_creates_entry_block_with_params() {
        let func = TirFunction::new("add".into(), vec![TirType::I64, TirType::I64], TirType::I64);

        assert_eq!(func.name, "add");
        assert_eq!(func.entry_block, BlockId(0));
        assert_eq!(func.next_value, 2);
        assert_eq!(func.next_block, 1);

        let entry = &func.blocks[&func.entry_block];
        assert_eq!(entry.args.len(), 2);
        assert_eq!(entry.args[0].ty, TirType::I64);
        assert_eq!(entry.args[1].id, ValueId(1));
    }

    #[test]
    fn function_fresh_ids_increment() {
        let mut func = TirFunction::new("f".into(), vec![], TirType::None);
        let v0 = func.fresh_value();
        let v1 = func.fresh_value();
        assert_eq!(v0, ValueId(0));
        assert_eq!(v1, ValueId(1));

        let b1 = func.fresh_block();
        let b2 = func.fresh_block();
        assert_eq!(b1, BlockId(1));
        assert_eq!(b2, BlockId(2));
    }

    #[test]
    fn function_with_multiple_blocks() {
        let mut func = TirFunction::new("branch_example".into(), vec![TirType::Bool], TirType::I64);

        // Create two successor blocks.
        let then_id = func.fresh_block();
        let else_id = func.fresh_block();

        let ret_val_then = func.fresh_value();
        let ret_val_else = func.fresh_value();

        let then_block = TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_val_then],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), crate::tir::ops::AttrValue::Int(1));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![ret_val_then],
            },
        };

        let else_block = TirBlock {
            id: else_id,
            args: vec![],
            ops: vec![TirOp {
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_val_else],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert("value".into(), crate::tir::ops::AttrValue::Int(0));
                    m
                },
                source_span: None,
            }],
            terminator: Terminator::Return {
                values: vec![ret_val_else],
            },
        };

        // Patch entry block terminator to branch.
        let cond = ValueId(0); // first param
        func.blocks.get_mut(&func.entry_block).unwrap().terminator = Terminator::CondBranch {
            cond,
            then_block: then_id,
            then_args: vec![],
            else_block: else_id,
            else_args: vec![],
        };

        func.blocks.insert(then_id, then_block);
        func.blocks.insert(else_id, else_block);

        assert_eq!(func.blocks.len(), 3);
        assert!(func.blocks.contains_key(&then_id));
        assert!(func.blocks.contains_key(&else_id));
    }

    #[test]
    fn module_holds_functions() {
        let f1 = TirFunction::new("a".into(), vec![], TirType::None);
        let f2 = TirFunction::new("b".into(), vec![TirType::I64], TirType::I64);
        let module = TirModule {
            name: "test_module".into(),
            functions: vec![f1, f2],
        };
        assert_eq!(module.name, "test_module");
        assert_eq!(module.functions.len(), 2);
    }
}
