//! Human-readable LIR dump in MLIR-like syntax.
//!
//! LIR is a lowering artifact, not TIR vocabulary. Keeping this printer separate
//! from `tir::printer` lets the TIR printer move with the future `molt-ir`
//! vocabulary crate without importing lowering-owned LIR types.

use super::blocks::BlockId;
use super::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use super::printer::{print_attr_value, print_dialect, print_opcode, print_type};
use super::types::TirType;
use super::values::ValueId;

/// Print a representation-aware LIR function in MLIR-like syntax.
pub fn print_lir_function(func: &LirFunction) -> String {
    let mut out = String::new();
    let params = func
        .blocks
        .get(&func.entry_block)
        .map(|entry| {
            entry
                .args
                .iter()
                .map(print_lir_value)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    out.push_str(&format!(
        "lir.func @{}({}) -> {} {{\n",
        func.name,
        params,
        print_lir_return_types(&func.return_types)
    ));

    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| {
        if *b == func.entry_block {
            u32::MIN
        } else {
            b.0
        }
    });

    for bid in block_ids {
        if let Some(block) = func.blocks.get(&bid) {
            out.push_str(&print_lir_block(block));
        }
    }

    out.push('}');
    out
}

fn print_lir_block(block: &LirBlock) -> String {
    let mut out = String::new();
    if block.args.is_empty() {
        out.push_str(&format!("  ^{}:\n", block.id));
    } else {
        let args = block
            .args
            .iter()
            .map(print_lir_value)
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("  ^{}({}):\n", block.id, args));
    }

    for op in &block.ops {
        out.push_str(&format!("    {}\n", print_lir_op(op)));
    }
    out.push_str(&format!(
        "    {}\n",
        print_lir_terminator(&block.terminator)
    ));
    out
}

fn print_lir_op(op: &LirOp) -> String {
    let results_str = if op.result_values.is_empty() {
        String::new()
    } else if op.result_values.len() == 1 {
        format!("{} = ", print_lir_value(&op.result_values[0]))
    } else {
        let values = op
            .result_values
            .iter()
            .map(print_lir_value)
            .collect::<Vec<_>>()
            .join(", ");
        format!("({}) = ", values)
    };
    let operands_str = op
        .tir_op
        .operands
        .iter()
        .map(|v| format!("{v}"))
        .collect::<Vec<_>>()
        .join(", ");
    let attrs_str = if op.tir_op.attrs.is_empty() {
        String::new()
    } else {
        let mut pairs: Vec<_> = op.tir_op.attrs.iter().collect();
        pairs.sort_by_key(|(k, _)| k.as_str());
        let inner = pairs
            .iter()
            .map(|(k, v)| format!("{}: {}", k, print_attr_value(v)))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" {{{}}}", inner)
    };

    if operands_str.is_empty() {
        format!(
            "{}{}.{}{}",
            results_str,
            print_dialect(op.tir_op.dialect),
            print_opcode(&op.tir_op.opcode),
            attrs_str
        )
    } else {
        format!(
            "{}{}.{} {}{}",
            results_str,
            print_dialect(op.tir_op.dialect),
            print_opcode(&op.tir_op.opcode),
            operands_str,
            attrs_str
        )
    }
}

fn print_lir_terminator(terminator: &LirTerminator) -> String {
    match terminator {
        LirTerminator::Branch { target, args } => {
            format!("br ^{}({})", target, print_value_list(args))
        }
        LirTerminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => format!(
            "cond_br {} ^{}({}) ^{}({})",
            cond,
            then_block,
            print_value_list(then_args),
            else_block,
            print_value_list(else_args)
        ),
        LirTerminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => {
            let cases = cases
                .iter()
                .map(|(case, block, args)| {
                    format!("{case}: ^{}({})", block, print_value_list(args))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "switch {} [{}] default ^{}({})",
                value,
                cases,
                default,
                print_value_list(default_args)
            )
        }
        LirTerminator::StateDispatch {
            cases,
            default,
            default_args,
        } => {
            let cases = cases
                .iter()
                .map(|(case, block, args)| {
                    format!("{case}: ^{}({})", block, print_value_list(args))
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!(
                "state_dispatch [{}] default ^{}({})",
                cases,
                default,
                print_value_list(default_args)
            )
        }
        LirTerminator::Return { values } => format!("return {}", print_value_list(values)),
        LirTerminator::Unreachable => "unreachable".to_string(),
    }
}

fn print_lir_return_types(types: &[TirType]) -> String {
    match types {
        [] => "()".to_string(),
        [only] => print_type(only),
        many => {
            let inner = many.iter().map(print_type).collect::<Vec<_>>().join(", ");
            format!("({inner})")
        }
    }
}

fn print_lir_value(value: &LirValue) -> String {
    format!(
        "{}: {} [{}]",
        value.id,
        print_type(&value.ty),
        print_lir_repr(value.repr)
    )
}

fn print_lir_repr(repr: LirRepr) -> &'static str {
    match repr {
        LirRepr::DynBox => "dynbox",
        LirRepr::Ref64 => "ref64",
        LirRepr::I64 => "i64",
        LirRepr::F64 => "f64",
        LirRepr::Bool1 => "bool1",
    }
}

fn print_value_list(values: &[ValueId]) -> String {
    values
        .iter()
        .map(|v| format!("{v}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn print_lir_function_renders_repr_annotated_signature() {
        let entry = BlockId(0);
        let arg = LirValue {
            id: ValueId(0),
            ty: TirType::I64,
            repr: LirRepr::I64,
        };
        let block = LirBlock {
            id: entry,
            args: vec![arg],
            ops: Vec::new(),
            terminator: LirTerminator::Return {
                values: vec![ValueId(0)],
            },
        };
        let func = LirFunction {
            name: "id_i64".to_string(),
            param_names: vec!["x".to_string()],
            param_types: vec![TirType::I64],
            return_types: vec![TirType::I64],
            blocks: HashMap::from([(entry, block)]),
            entry_block: entry,
            label_id_map: HashMap::new(),
        };

        let rendered = print_lir_function(&func);

        assert!(rendered.contains("lir.func @id_i64(%0: i64 [i64]) -> i64"));
        assert!(rendered.contains("return %0"));
    }
}
