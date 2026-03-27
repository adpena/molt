//! MLIR compatibility layer for TIR.
//!
//! Provides serialization to MLIR textual format, dialect specification
//! documentation, and compatibility validation. Ensures TIR can be
//! mechanically translated to MLIR when the migration happens.

use std::collections::HashSet;
use std::fmt::Write;

use super::blocks::{BlockId, Terminator};
use super::function::TirFunction;
use super::ops::{Dialect, OpCode};
use super::types::TirType;
use super::values::ValueId;

/// Serialize a TIR function to MLIR textual format.
pub fn to_mlir_text(func: &TirFunction) -> String {
    let mut out = String::with_capacity(4096);
    let params: Vec<String> = func
        .param_types
        .iter()
        .enumerate()
        .map(|(i, ty)| format!("%arg{}: {}", i, mlir_type(ty)))
        .collect();
    let ret = mlir_type(&func.return_type);
    writeln!(
        out,
        "func.func @{}({}) -> {} {{",
        func.name,
        params.join(", "),
        ret
    )
    .unwrap();

    let mut block_ids: Vec<BlockId> = func.blocks.keys().copied().collect();
    block_ids.sort_by_key(|b| b.0);

    for bid in &block_ids {
        let block = &func.blocks[bid];
        let args: Vec<String> = block
            .args
            .iter()
            .map(|a| format!("%{}: {}", a.id.0, mlir_type(&a.ty)))
            .collect();
        if args.is_empty() {
            writeln!(out, "^bb{}:", bid.0).unwrap();
        } else {
            writeln!(out, "^bb{}({}):", bid.0, args.join(", ")).unwrap();
        }

        for op in &block.ops {
            let d = mlir_dialect(&op.dialect);
            let o = mlir_opcode(&op.opcode);
            let operands: Vec<String> = op.operands.iter().map(|v| format!("%{}", v.0)).collect();
            let results: Vec<String> = op.results.iter().map(|v| format!("%{}", v.0)).collect();
            if results.is_empty() {
                writeln!(out, "  \"{d}.{o}\"({}) : () -> ()", operands.join(", ")).unwrap();
            } else {
                writeln!(
                    out,
                    "  {} = \"{d}.{o}\"({}) : ({}) -> ({})",
                    results.join(", "),
                    operands.join(", "),
                    vec!["i64"; operands.len()].join(", "),
                    vec!["i64"; results.len()].join(", ")
                )
                .unwrap();
            }
        }

        match &block.terminator {
            Terminator::Return { values } => {
                let v: Vec<String> = values.iter().map(|v| format!("%{}", v.0)).collect();
                if v.is_empty() {
                    writeln!(out, "  return").unwrap();
                } else {
                    writeln!(out, "  return {} : {ret}", v.join(", ")).unwrap();
                }
            }
            Terminator::Branch { target, args } => {
                let a: Vec<String> = args.iter().map(|v| format!("%{}", v.0)).collect();
                writeln!(out, "  br ^bb{}({})", target.0, a.join(", ")).unwrap();
            }
            Terminator::CondBranch {
                cond,
                then_block,
                then_args,
                else_block,
                else_args,
            } => {
                let ta: Vec<String> = then_args.iter().map(|v| format!("%{}", v.0)).collect();
                let ea: Vec<String> = else_args.iter().map(|v| format!("%{}", v.0)).collect();
                writeln!(
                    out,
                    "  cond_br %{}, ^bb{}({}), ^bb{}({})",
                    cond.0,
                    then_block.0,
                    ta.join(", "),
                    else_block.0,
                    ea.join(", ")
                )
                .unwrap();
            }
            Terminator::Switch {
                value,
                cases,
                default,
                default_args,
            } => {
                let da: Vec<String> = default_args.iter().map(|v| format!("%{}", v.0)).collect();
                write!(out, "  switch %{} [", value.0).unwrap();
                for (val, target, args) in cases {
                    let a: Vec<String> = args.iter().map(|v| format!("%{}", v.0)).collect();
                    write!(out, "{}: ^bb{}({}), ", val, target.0, a.join(", ")).unwrap();
                }
                writeln!(out, "default: ^bb{}({})]", default.0, da.join(", ")).unwrap();
            }
            Terminator::Unreachable => {
                writeln!(out, "  \"molt.unreachable\"() : () -> ()").unwrap();
            }
        }
    }
    writeln!(out, "}}").unwrap();
    out
}

/// Validate MLIR compatibility requirements.
pub fn validate_mlir_compat(func: &TirFunction) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if !func.blocks.contains_key(&func.entry_block) {
        errors.push(format!(
            "entry block ^bb{} does not exist",
            func.entry_block.0
        ));
    }
    let mut defined: HashSet<ValueId> = HashSet::new();
    for (bid, block) in &func.blocks {
        for arg in &block.args {
            if !defined.insert(arg.id) {
                errors.push(format!("^bb{}: duplicate ValueId %{}", bid.0, arg.id.0));
            }
        }
        for op in &block.ops {
            for &r in &op.results {
                if !defined.insert(r) {
                    errors.push(format!("^bb{}: duplicate ValueId %{}", bid.0, r.0));
                }
            }
        }
        for target in terminator_targets(&block.terminator) {
            if !func.blocks.contains_key(&target) {
                errors.push(format!("^bb{}: target ^bb{} missing", bid.0, target.0));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// MLIR ODS dialect specification for the molt dialect.
pub fn dialect_spec() -> String {
    r#"// Molt Dialect — MLIR ODS Specification
def Molt_AddOp : Molt_Op<"add", [Pure]> { let arguments = (ins AnyType:$lhs, AnyType:$rhs); let results = (outs AnyType:$result); }
def Molt_BoxOp : Molt_Op<"box", [Pure]> { let arguments = (ins AnyType:$value); let results = (outs I64:$boxed); }
def Molt_UnboxOp : Molt_Op<"unbox", [Pure]> { let arguments = (ins I64:$boxed); let results = (outs AnyType:$value); }
def Molt_CallOp : Molt_Op<"call"> { let arguments = (ins StrAttr:$callee, Variadic<AnyType>:$operands); let results = (outs AnyType:$result); }
def Molt_DeoptOp : Molt_Op<"deopt"> { let arguments = (ins StrAttr:$target_func, Variadic<AnyType>:$live_values); }
def MoltGpu_LaunchOp : MoltGpu_Op<"launch"> { let regions = (region SizedRegion<1>:$body); }
def MoltGpu_ThreadIdOp : MoltGpu_Op<"thread_id", [Pure]> { let results = (outs I64:$id); }
"#.to_string()
}

fn terminator_targets(term: &Terminator) -> Vec<BlockId> {
    match term {
        Terminator::Branch { target, .. } => vec![*target],
        Terminator::CondBranch {
            then_block,
            else_block,
            ..
        } => vec![*then_block, *else_block],
        Terminator::Switch { cases, default, .. } => {
            let mut t: Vec<BlockId> = cases.iter().map(|(_, b, _)| *b).collect();
            t.push(*default);
            t
        }
        _ => vec![],
    }
}

fn mlir_type(ty: &TirType) -> &'static str {
    match ty {
        TirType::I64 | TirType::BigInt => "i64",
        TirType::F64 => "f64",
        TirType::Bool => "i1",
        TirType::None | TirType::DynBox => "i64",
        TirType::Str | TirType::Bytes | TirType::Ptr(_) => "!molt.ptr",
        TirType::Never => "none",
        _ => "i64",
    }
}

fn mlir_dialect(d: &Dialect) -> &'static str {
    match d {
        Dialect::Molt => "molt",
        Dialect::Scf => "scf",
        Dialect::Gpu => "molt.gpu",
        Dialect::Par => "molt.par",
        Dialect::Simd => "molt.simd",
    }
}

fn mlir_opcode(op: &OpCode) -> &'static str {
    match op {
        OpCode::Add => "add",
        OpCode::Sub => "sub",
        OpCode::Mul => "mul",
        OpCode::Div => "div",
        OpCode::FloorDiv => "floordiv",
        OpCode::Mod => "mod",
        OpCode::Pow => "pow",
        OpCode::Neg => "neg",
        OpCode::Pos => "pos",
        OpCode::Eq => "eq",
        OpCode::Ne => "ne",
        OpCode::Lt => "lt",
        OpCode::Le => "le",
        OpCode::Gt => "gt",
        OpCode::Ge => "ge",
        OpCode::Is => "is",
        OpCode::IsNot => "is_not",
        OpCode::In => "in",
        OpCode::NotIn => "not_in",
        OpCode::BitAnd => "bit_and",
        OpCode::BitOr => "bit_or",
        OpCode::BitXor => "bit_xor",
        OpCode::BitNot => "bit_not",
        OpCode::Shl => "shl",
        OpCode::Shr => "shr",
        OpCode::And => "and",
        OpCode::Or => "or",
        OpCode::Not => "not",
        OpCode::Alloc => "alloc",
        OpCode::StackAlloc => "stack_alloc",
        OpCode::Free => "free",
        OpCode::LoadAttr => "load_attr",
        OpCode::StoreAttr => "store_attr",
        OpCode::DelAttr => "del_attr",
        OpCode::Index => "index",
        OpCode::StoreIndex => "store_index",
        OpCode::DelIndex => "del_index",
        OpCode::Call => "call",
        OpCode::CallMethod => "call_method",
        OpCode::CallBuiltin => "call_builtin",
        OpCode::BoxVal => "box",
        OpCode::UnboxVal => "unbox",
        OpCode::TypeGuard => "type_guard",
        OpCode::IncRef => "inc_ref",
        OpCode::DecRef => "dec_ref",
        OpCode::BuildList => "build_list",
        OpCode::BuildDict => "build_dict",
        OpCode::BuildTuple => "build_tuple",
        OpCode::BuildSet => "build_set",
        OpCode::BuildSlice => "build_slice",
        OpCode::GetIter => "get_iter",
        OpCode::IterNext => "iter_next",
        OpCode::ForIter => "for_iter",
        OpCode::Yield => "yield",
        OpCode::YieldFrom => "yield_from",
        OpCode::Raise => "raise",
        OpCode::CheckException => "check_exception",
        OpCode::TryStart => "try_start",
        OpCode::TryEnd => "try_end",
        OpCode::StateBlockStart => "state_block_start",
        OpCode::StateBlockEnd => "state_block_end",
        OpCode::ConstInt => "const_int",
        OpCode::ConstFloat => "const_float",
        OpCode::ConstStr => "const_str",
        OpCode::ConstBool => "const_bool",
        OpCode::ConstNone => "const_none",
        OpCode::ConstBytes => "const_bytes",
        OpCode::Copy => "copy",
        OpCode::Import => "import",
        OpCode::ImportFrom => "import_from",
        OpCode::ScfIf => "if",
        OpCode::ScfFor => "for",
        OpCode::ScfWhile => "while",
        OpCode::ScfYield => "yield",
        OpCode::Deopt => "deopt",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::TirBlock;
    use crate::tir::ops::{AttrDict, TirOp};

    fn make_add_func() -> TirFunction {
        let mut func =
            TirFunction::new("add".into(), vec![TirType::I64, TirType::I64], TirType::I64);
        let v2 = func.fresh_value();
        let entry = func.blocks.get_mut(&func.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::Add,
            operands: vec![ValueId(0), ValueId(1)],
            results: vec![v2],
            attrs: AttrDict::new(),
            source_span: None,
        });
        entry.terminator = Terminator::Return { values: vec![v2] };
        func
    }

    #[test]
    fn mlir_text_contains_func() {
        let text = to_mlir_text(&make_add_func());
        assert!(text.contains("func.func @add"));
        assert!(text.contains("\"molt.add\""));
        assert!(text.contains("return"));
    }

    #[test]
    fn mlir_text_has_block() {
        assert!(to_mlir_text(&make_add_func()).contains("^bb0"));
    }

    #[test]
    fn validate_valid_passes() {
        assert!(validate_mlir_compat(&make_add_func()).is_ok());
    }

    #[test]
    fn validate_missing_entry_fails() {
        let mut f = make_add_func();
        f.entry_block = BlockId(999);
        assert!(validate_mlir_compat(&f).is_err());
    }

    #[test]
    fn validate_duplicate_value_fails() {
        let mut f = make_add_func();
        let entry = f.blocks.get_mut(&f.entry_block).unwrap();
        entry.ops.push(TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ValueId(0)],
            attrs: AttrDict::new(),
            source_span: None,
        });
        assert!(validate_mlir_compat(&f).is_err());
    }

    #[test]
    fn dialect_spec_has_key_ops() {
        let s = dialect_spec();
        assert!(s.contains("Molt_AddOp"));
        assert!(s.contains("MoltGpu_LaunchOp"));
    }

    #[test]
    fn mlir_text_conditional() {
        let mut f = TirFunction::new("cond".into(), vec![TirType::Bool], TirType::I64);
        let tb = f.fresh_block();
        let eb = f.fresh_block();
        let v1 = f.fresh_value();
        let v2 = f.fresh_value();
        f.blocks.get_mut(&f.entry_block).unwrap().terminator = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: tb,
            then_args: vec![],
            else_block: eb,
            else_args: vec![],
        };
        f.blocks.insert(
            tb,
            TirBlock {
                id: tb,
                args: vec![],
                terminator: Terminator::Return { values: vec![v1] },
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v1],
                    attrs: AttrDict::new(),
                    source_span: None,
                }],
            },
        );
        f.blocks.insert(
            eb,
            TirBlock {
                id: eb,
                args: vec![],
                terminator: Terminator::Return { values: vec![v2] },
                ops: vec![TirOp {
                    dialect: Dialect::Molt,
                    opcode: OpCode::ConstInt,
                    operands: vec![],
                    results: vec![v2],
                    attrs: AttrDict::new(),
                    source_span: None,
                }],
            },
        );
        let text = to_mlir_text(&f);
        assert!(text.contains("cond_br"));
        assert!(text.contains("^bb1"));
        assert!(text.contains("^bb2"));
    }
}
