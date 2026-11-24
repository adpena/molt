use crate::deterministic_hash::HashMap;

use super::blocks::{BlockId, LoopBreakKind, LoopRole, TirBlock};
use super::op_kinds_generated::{
    opcode_is_exception_handler_region_table, opcode_is_lowered_state_machine_body_table,
};
use super::ops::{AttrDict, AttrValue};
use super::types::TirType;
use super::values::ValueId;
use crate::ir::{ExecutionContextPolicy, FunctionReturnAbi, ParameterCustody};

mod block_retention;

/// Attribute projection of the typed SimpleIR physical partition fact.
pub const CODEGEN_PARTITION_ATTR: &str = "codegen_partition";
/// Function attr: on a 3.13+ target a PEP 667 frame-locals proxy may rebind
/// the function's locals (`FunctionIR::frame_locals_exposed`, HF-901).
pub const FRAME_LOCALS_EXPOSED_ATTR: &str = "frame_locals_exposed";

/// Attribute projection of the typed FunctionIR parameter custody fact: the
/// [`ParameterCustody::encode`] bytes, one per parameter of the declared
/// signature. Absent means every parameter is borrowed.
pub const PARAMETER_CUSTODY_ATTR: &str = "parameter_custody";

/// A function in TIR: a collection of basic blocks in SSA form.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct TirFunction {
    pub name: String,
    /// Frozen callable ABI; type refinement and body optimization preserve it.
    pub return_abi: FunctionReturnAbi,
    /// Target-neutral execution-context ABI preserved through cached TIR and
    /// every SimpleIR relift.
    #[serde(default)]
    pub execution_context: ExecutionContextPolicy,
    /// Original parameter names, aligned 1:1 with `param_types` and the entry
    /// block arguments. These are preserved through the TIR round-trip so
    /// backends do not have to recover parameter identity from synthetic
    /// `load_param` temporaries.
    pub param_names: Vec<String>,
    /// Parameter types (mapped 1:1 to entry block arguments).
    pub param_types: Vec<TirType>,
    /// The exact scalar type each parameter holds on every entry, one per
    /// parameter (`DynBox` where nothing is proven), or empty for a body any
    /// caller may enter. Only the type flow gives a specialized clone these
    /// facts, and only proven call sites whose arguments have these types
    /// enter it (design 78, sections 3.3 and 3.7). An annotation admits
    /// subclasses, so it never produces one.
    #[serde(default)]
    pub exact_param_types: Vec<TirType>,
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
    pub label_id_map: crate::deterministic_hash::HashMap<u32, i64>,
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
    /// Number of operations the backend walks, including one terminator per block.
    pub fn operation_count(&self) -> usize {
        self.blocks.values().map(|block| block.ops.len() + 1).sum()
    }

    pub fn is_codegen_partition(&self) -> bool {
        matches!(
            self.attrs.get(CODEGEN_PARTITION_ATTR),
            Some(super::ops::AttrValue::Bool(true))
        )
    }

    /// Whether a PEP 667 frame-locals proxy may rebind this function's
    /// locals (`FRAME_LOCALS_EXPOSED_ATTR`).
    pub fn frame_locals_exposed(&self) -> bool {
        matches!(
            self.attrs.get(FRAME_LOCALS_EXPOSED_ATTR),
            Some(super::ops::AttrValue::Bool(true))
        )
    }

    /// Custody of the parameter at `position` of the declared signature, for a
    /// body and an extern declaration alike. A projection that does not name
    /// every parameter is malformed and is never guessed.
    pub fn parameter_custody(&self, position: usize) -> ParameterCustody {
        let arity = self.param_types.len();
        let custody = match self.attrs.get(PARAMETER_CUSTODY_ATTR) {
            None => return ParameterCustody::Borrowed,
            Some(AttrValue::Bytes(encoded)) => ParameterCustody::decode(encoded, arity, position),
            Some(_) => None,
        };
        custody.unwrap_or_else(|| {
            panic!(
                "{}: malformed {PARAMETER_CUSTODY_ATTR} for parameter {position} of {arity}",
                self.name
            )
        })
    }

    /// Project `custody`, one entry per parameter. All-borrowed custody is the
    /// absent attribute, so the fact has one encoding.
    pub fn set_parameter_custody(&mut self, custody: &[ParameterCustody]) {
        assert_eq!(
            custody.len(),
            self.param_types.len(),
            "{}: parameter custody must name every parameter",
            self.name
        );
        match ParameterCustody::encode(custody) {
            Some(encoded) => {
                self.attrs
                    .insert(PARAMETER_CUSTODY_ATTR.into(), AttrValue::Bytes(encoded));
            }
            None => {
                self.attrs.remove(PARAMETER_CUSTODY_ATTR);
            }
        }
    }

    /// Create a new function with a single empty entry block.
    pub fn new(
        name: String,
        param_types: Vec<TirType>,
        return_type: TirType,
        return_abi: FunctionReturnAbi,
    ) -> Self {
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

        let mut value_types = HashMap::default();
        for arg in &entry.args {
            value_types.insert(arg.id, arg.ty.clone());
        }

        let mut blocks = HashMap::default();
        blocks.insert(entry_id, entry);

        Self {
            name,
            return_abi,
            execution_context: ExecutionContextPolicy::None,
            param_names: param_types
                .iter()
                .enumerate()
                .map(|(idx, _)| format!("p{idx}"))
                .collect(),
            param_types,
            exact_param_types: Vec::new(),
            return_type,
            blocks,
            entry_block: entry_id,
            next_value,
            next_block: 1,
            attrs: AttrDict::default(),
            value_types,
            has_exception_handling: false,
            label_id_map: HashMap::default(),
            loop_roles: HashMap::default(),
            loop_pairs: HashMap::default(),
            loop_break_kinds: HashMap::default(),
            loop_cond_blocks: HashMap::default(),
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

    /// Move the structural loop boundaries that describe a block's original
    /// terminator to the fresh block that now executes it. Loop headers and
    /// incoming labels still name the entry side of the split.
    pub fn move_loop_terminator(&mut self, from: BlockId, to: BlockId) {
        for boundary in self
            .loop_cond_blocks
            .values_mut()
            .chain(self.loop_pairs.values_mut())
        {
            if *boundary == from {
                *boundary = to;
            }
        }
        if self.loop_roles.get(&from) == Some(&LoopRole::LoopEnd) {
            self.loop_roles.remove(&from);
            self.loop_roles.insert(to, LoopRole::LoopEnd);
        }
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
    /// exception-observation change (`ab323ec10`) virtually every real function
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
        self.blocks.values().any(|block| {
            block
                .ops
                .iter()
                .any(|op| opcode_is_exception_handler_region_table(op.opcode))
        })
    }

    /// True for a coroutine/generator poll activation with saved-state dispatch.
    /// Resume edges belong to the CFG. Persistence belongs to explicit frame
    /// stores/loads; terminal DropInsertion exposes suspension as ordinary
    /// returns and releases invocation-local owners through the shared analysis.
    pub fn has_state_machine(&self) -> bool {
        use super::blocks::Terminator;
        self.blocks.values().any(|block| {
            // The `state_switch` dispatch is now a first-class `StateDispatch`
            // terminator (not a body op), so detect it there; the suspend ops
            // (`StateYield`/`StateTransition`/`Chan*Yield`) remain body ops.
            // AllocTask only constructs a generator/coroutine and is not
            // evidence that the containing function itself is re-entrant.
            matches!(block.terminator, Terminator::StateDispatch { .. })
                || block
                    .ops
                    .iter()
                    .any(|op| opcode_is_lowered_state_machine_body_table(op.opcode))
        })
    }
}

/// A module: a collection of TIR functions.
///
/// `table` is the function table of the document the module came from,
/// bodies and extern rows alike; `ids[i]` is the row of `functions[i]`
/// (HF-847 slice 1). Call edges name callees by those ids.
#[derive(Debug, Clone)]
pub struct TirModule {
    pub name: String,
    pub functions: Vec<TirFunction>,
    pub table: crate::FunctionTable,
    pub ids: Vec<crate::FunctionId>,
}

impl TirModule {
    /// A module whose table is exactly its bodies, in order: `functions[i]`
    /// is row `i`. A document with extern rows uses [`Self::from_document`].
    pub fn with_own_table(name: impl Into<String>, functions: Vec<TirFunction>) -> Self {
        let rows = functions
            .iter()
            .map(|function| crate::FunctionIR {
                name: function.name.clone(),
                ..crate::FunctionIR::default()
            })
            .collect::<Vec<_>>();
        let ids = (0..functions.len())
            .map(crate::FunctionId::from_index)
            .collect();
        Self {
            name: name.into(),
            functions,
            table: crate::FunctionTable::of(&rows),
            ids,
        }
    }

    /// Decode for hand-built TIR (fixtures, tools): the table is the bodies in
    /// order plus one extern row per other symbol a function-reference op
    /// spells, and each such op's `s_value` moves into `callee`, as SimpleIR
    /// decode does (HF-847 slice 1, D2). Ops of every other kind keep their
    /// attributes.
    pub fn decode_spelled(name: impl Into<String>, mut functions: Vec<TirFunction>) -> Self {
        let mut rows: Vec<crate::FunctionIR> = functions
            .iter()
            .map(|function| crate::FunctionIR {
                name: function.name.clone(),
                ..crate::FunctionIR::default()
            })
            .collect();
        let mut by_symbol: HashMap<String, crate::FunctionId> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| (row.name.clone(), crate::FunctionId::from_index(index)))
            .collect();
        for function in &mut functions {
            for block in function.blocks.values_mut() {
                for op in &mut block.ops {
                    let kind = op.origin.or_else(|| {
                        super::op_kinds_generated::opcode_canonical_kind_table(op.opcode)
                    });
                    if kind.is_none_or(|kind| {
                        kind.function_reference_role() == crate::FunctionReferenceRole::None
                    }) || op.callee.is_some()
                    {
                        continue;
                    }
                    let Some(AttrValue::Str(symbol)) = op.attrs.get("s_value") else {
                        continue;
                    };
                    let next = crate::FunctionId::from_index(rows.len());
                    let id = *by_symbol.entry(symbol.clone()).or_insert_with(|| {
                        rows.push(crate::FunctionIR {
                            name: symbol.clone(),
                            is_extern: true,
                            ..crate::FunctionIR::default()
                        });
                        next
                    });
                    op.attrs.remove("s_value");
                    op.callee = Some(id);
                }
            }
        }
        let ids = (0..functions.len())
            .map(crate::FunctionId::from_index)
            .collect();
        Self {
            name: name.into(),
            functions,
            table: crate::FunctionTable::of(&rows),
            ids,
        }
    }

    /// A module over `table` holding the bodies `functions`, each with its row.
    pub fn from_document(
        name: impl Into<String>,
        table: crate::FunctionTable,
        functions: Vec<(crate::FunctionId, TirFunction)>,
    ) -> Self {
        let (ids, functions) = functions.into_iter().unzip();
        Self {
            name: name.into(),
            functions,
            table,
            ids,
        }
    }

    /// Row -> body index, for the rows that have a body in this module.
    pub fn body_index(&self) -> Vec<Option<usize>> {
        let mut index = vec![None; self.table.len()];
        for (position, id) in self.ids.iter().enumerate() {
            index[id.index()] = Some(position);
        }
        index
    }
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
        let func = TirFunction::new(
            "add".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
            crate::FunctionReturnAbi::Value,
        );

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
        let mut func = TirFunction::new(
            "f".into(),
            vec![],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );
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
        let mut func = TirFunction::new(
            "branch_example".into(),
            vec![TirType::Bool],
            TirType::I64,
            crate::FunctionReturnAbi::Value,
        );

        // Create two successor blocks.
        let then_id = func.fresh_block();
        let else_id = func.fresh_block();

        let ret_val_then = func.fresh_value();
        let ret_val_else = func.fresh_value();

        let then_block = TirBlock {
            id: then_id,
            args: vec![],
            ops: vec![TirOp {
                origin: None,
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_val_then],
                attrs: {
                    let mut m = AttrDict::default();
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
                origin: None,
                runtime: None,
                callee: None,
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_val_else],
                attrs: {
                    let mut m = AttrDict::default();
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
        let f1 = TirFunction::new(
            "a".into(),
            vec![],
            TirType::None,
            crate::FunctionReturnAbi::Void,
        );
        let f2 = TirFunction::new(
            "b".into(),
            vec![TirType::I64],
            TirType::I64,
            crate::FunctionReturnAbi::Value,
        );
        let module = TirModule::with_own_table("test_module", vec![f1, f2]);
        assert_eq!(module.name, "test_module");
        assert_eq!(module.functions.len(), 2);
    }
}
