use std::collections::HashMap;

use super::blocks::{BlockId, TirBlock};
use super::types::TirType;
use super::values::ValueId;

/// A function in TIR: a collection of basic blocks in SSA form.
#[derive(Debug)]
pub struct TirFunction {
    pub name: String,
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
                TirValue {
                    id,
                    ty: ty.clone(),
                }
            })
            .collect();

        let entry = TirBlock {
            id: entry_id,
            args,
            ops: Vec::new(),
            terminator: Terminator::Unreachable,
        };

        let mut blocks = HashMap::new();
        blocks.insert(entry_id, entry);

        Self {
            name,
            param_types,
            return_type,
            blocks,
            entry_block: entry_id,
            next_value,
            next_block: 1,
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
}

/// A module: a collection of TIR functions.
#[derive(Debug)]
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
        let func = TirFunction::new(
            "add".into(),
            vec![TirType::I64, TirType::I64],
            TirType::I64,
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
        let mut func = TirFunction::new(
            "branch_example".into(),
            vec![TirType::Bool],
            TirType::I64,
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
                dialect: Dialect::Molt,
                opcode: OpCode::ConstInt,
                operands: vec![],
                results: vec![ret_val_then],
                attrs: {
                    let mut m = AttrDict::new();
                    m.insert(
                        "value".into(),
                        crate::tir::ops::AttrValue::Int(1),
                    );
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
                    m.insert(
                        "value".into(),
                        crate::tir::ops::AttrValue::Int(0),
                    );
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
