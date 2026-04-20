//! Human-readable TIR dump in MLIR-like syntax.
//!
//! Enable with `TIR_DUMP=1` environment variable. This is a diagnostic tool —
//! correctness and readability matter more than performance.

use super::blocks::{BlockId, Terminator, TirBlock};
use super::function::{TirFunction, TirModule};
use super::lir::{LirBlock, LirFunction, LirOp, LirRepr, LirTerminator, LirValue};
use super::ops::{AttrValue, Dialect, OpCode, TirOp};
use super::types::{FuncSignature, TirType};
use super::values::ValueId;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Returns true if `TIR_DUMP=1` is set in the environment.
pub fn tir_dump_enabled() -> bool {
    std::env::var("TIR_DUMP").map(|v| v == "1").unwrap_or(false)
}

/// Print a TirModule in MLIR-like syntax.
pub fn print_module(module: &TirModule) -> String {
    let mut out = String::new();
    out.push_str(&format!("module @{} {{\n", module.name));
    for func in &module.functions {
        let func_str = print_function(func);
        // Indent each line of the function by 2 spaces.
        for line in func_str.lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push('}');
    out
}

/// Print a TirFunction in MLIR-like syntax.
pub fn print_function(func: &TirFunction) -> String {
    let mut out = String::new();

    // Build param list from entry block arguments.
    let entry = func.blocks.get(&func.entry_block);
    let params = if let Some(entry_block) = entry {
        entry_block
            .args
            .iter()
            .map(|arg| format!("{}: {}", arg.id, print_type(&arg.ty)))
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        // Fall back to param_types if no entry block (shouldn't happen in valid IR).
        func.param_types
            .iter()
            .enumerate()
            .map(|(i, ty)| format!("%{i}: {}", print_type(ty)))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let ret = print_type(&func.return_type);
    out.push_str(&format!("func @{}({}) -> {} {{\n", func.name, params, ret));

    // Print blocks in a deterministic order: entry first, then by BlockId.
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
            out.push_str(&print_block(block));
        }
    }

    out.push('}');
    out
}

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

/// Print a single basic block.
fn print_block(block: &TirBlock) -> String {
    let mut out = String::new();

    // Block header: ^bb<id>(<args>):
    if block.args.is_empty() {
        out.push_str(&format!("  ^{}:\n", block.id));
    } else {
        let args = block
            .args
            .iter()
            .map(|arg| format!("{}: {}", arg.id, print_type(&arg.ty)))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("  ^{}({}):\n", block.id, args));
    }

    // Ops.
    for op in &block.ops {
        out.push_str(&format!("    {}\n", print_op(op)));
    }

    // Terminator.
    out.push_str(&format!("    {}\n", print_terminator(&block.terminator)));

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

/// Print a single TirOp.
fn print_op(op: &TirOp) -> String {
    let dialect_prefix = print_dialect(op.dialect);
    let opcode_name = print_opcode(&op.opcode);

    // Results.
    let results_str = if op.results.is_empty() {
        String::new()
    } else {
        let r = op
            .results
            .iter()
            .map(|v| format!("{v}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{r} = ")
    };

    // Operands.
    let operands_str = op
        .operands
        .iter()
        .map(|v| format!("{v}"))
        .collect::<Vec<_>>()
        .join(", ");

    // Attributes.
    let attrs_str = if op.attrs.is_empty() {
        String::new()
    } else {
        let mut pairs: Vec<(&String, &AttrValue)> = op.attrs.iter().collect();
        pairs.sort_by_key(|(k, _)| k.as_str());
        let inner = pairs
            .iter()
            .map(|(k, v)| format!("{}: {}", k, print_attr_value(v)))
            .collect::<Vec<_>>()
            .join(", ");
        format!(" {{{}}}", inner)
    };

    // Source span.
    let span_str = if let Some((lo, hi)) = op.source_span {
        format!(" // span:{lo}..{hi}")
    } else {
        String::new()
    };

    if operands_str.is_empty() {
        format!(
            "{}{}.{}{}{}",
            results_str, dialect_prefix, opcode_name, attrs_str, span_str
        )
    } else {
        format!(
            "{}{}.{} {}{}{}",
            results_str, dialect_prefix, opcode_name, operands_str, attrs_str, span_str
        )
    }
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
        let mut pairs: Vec<(&String, &AttrValue)> = op.tir_op.attrs.iter().collect();
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
        LirTerminator::Return { values } => format!("return {}", print_value_list(values)),
        LirTerminator::Unreachable => "unreachable".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

/// Format a TirType as a string.
pub fn print_type(ty: &TirType) -> String {
    match ty {
        TirType::I64 => "i64".to_string(),
        TirType::F64 => "f64".to_string(),
        TirType::Bool => "bool".to_string(),
        TirType::None => "none".to_string(),
        TirType::Str => "str".to_string(),
        TirType::Bytes => "bytes".to_string(),
        TirType::List(inner) => format!("list<{}>", print_type(inner)),
        TirType::Dict(k, v) => format!("dict<{}, {}>", print_type(k), print_type(v)),
        TirType::Set(inner) => format!("set<{}>", print_type(inner)),
        TirType::Tuple(elems) => {
            let inner = elems.iter().map(print_type).collect::<Vec<_>>().join(", ");
            format!("tuple<{}>", inner)
        }
        TirType::Box(inner) => format!("box<{}>", print_type(inner)),
        TirType::DynBox => "dynbox".to_string(),
        TirType::Func(sig) => print_func_signature(sig),
        TirType::BigInt => "bigint".to_string(),
        TirType::Ptr(inner) => format!("ptr<{}>", print_type(inner)),
        TirType::Union(members) => {
            let inner = members
                .iter()
                .map(print_type)
                .collect::<Vec<_>>()
                .join(" | ");
            format!("({})", inner)
        }
        TirType::Never => "never".to_string(),
    }
}

fn print_func_signature(sig: &FuncSignature) -> String {
    let params = sig
        .params
        .iter()
        .map(print_type)
        .collect::<Vec<_>>()
        .join(", ");
    format!("func({}) -> {}", params, print_type(&sig.return_type))
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

/// Return the dialect namespace prefix for an op (e.g. "molt", "scf").
pub fn print_dialect(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Molt => "molt",
        Dialect::Scf => "scf",
        Dialect::Gpu => "gpu",
        Dialect::Par => "par",
        Dialect::Simd => "simd",
    }
}

/// Return the lowercase opcode name (e.g. "add", "const_int").
pub fn print_opcode(op: &OpCode) -> &'static str {
    match op {
        OpCode::Add => "add",
        OpCode::Sub => "sub",
        OpCode::Mul => "mul",
        OpCode::InplaceAdd => "inplace_add",
        OpCode::InplaceSub => "inplace_sub",
        OpCode::InplaceMul => "inplace_mul",
        OpCode::Div => "div",
        OpCode::FloorDiv => "floor_div",
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
        OpCode::Bool => "bool",
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
        OpCode::BoxVal => "box_val",
        OpCode::UnboxVal => "unbox_val",
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
        OpCode::IterNextUnboxed => "iter_next_unboxed",
        OpCode::ForIter => "for_iter",
        OpCode::AllocTask => "alloc_task",
        OpCode::StateSwitch => "state_switch",
        OpCode::StateTransition => "state_transition",
        OpCode::StateYield => "state_yield",
        OpCode::ChanSendYield => "chan_send_yield",
        OpCode::ChanRecvYield => "chan_recv_yield",
        OpCode::ClosureLoad => "closure_load",
        OpCode::ClosureStore => "closure_store",
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
        OpCode::WarnStderr => "warn_stderr",
    }
}

/// Format a block terminator.
pub fn print_terminator(term: &Terminator) -> String {
    match term {
        Terminator::Return { values } => {
            if values.is_empty() {
                "return".to_string()
            } else {
                let vals = values
                    .iter()
                    .map(|v| format!("{v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("return {}", vals)
            }
        }
        Terminator::Branch { target, args } => {
            if args.is_empty() {
                format!("br ^{}", target)
            } else {
                let arg_str = args
                    .iter()
                    .map(|v| format!("{v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("br ^{}({})", target, arg_str)
            }
        }
        Terminator::CondBranch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            let then_arg_str = if then_args.is_empty() {
                String::new()
            } else {
                let s = then_args
                    .iter()
                    .map(|v| format!("{v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", s)
            };
            let else_arg_str = if else_args.is_empty() {
                String::new()
            } else {
                let s = else_args
                    .iter()
                    .map(|v| format!("{v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", s)
            };
            format!(
                "cond_br {}, ^{}{}, ^{}{}",
                cond, then_block, then_arg_str, else_block, else_arg_str
            )
        }
        Terminator::Switch {
            value,
            cases,
            default,
            default_args,
        } => {
            let mut parts = vec![format!("switch {}", value)];
            for (case_val, target, args) in cases {
                let arg_str = if args.is_empty() {
                    String::new()
                } else {
                    let s = args
                        .iter()
                        .map(|v| format!("{v}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("({})", s)
                };
                parts.push(format!("  case {}: ^{}{}", case_val, target, arg_str));
            }
            let default_arg_str = if default_args.is_empty() {
                String::new()
            } else {
                let s = default_args
                    .iter()
                    .map(|v| format!("{v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("({})", s)
            };
            parts.push(format!("  default: ^{}{}", default, default_arg_str));
            parts.join("\n")
        }
        Terminator::Unreachable => "unreachable".to_string(),
    }
}

fn print_attr_value(val: &AttrValue) -> String {
    match val {
        AttrValue::Int(i) => i.to_string(),
        AttrValue::Float(f) => format!("{:?}", f),
        AttrValue::Str(s) => format!("{:?}", s),
        AttrValue::Bool(b) => b.to_string(),
        AttrValue::Bytes(bytes) => format!("bytes<{}>", bytes.len()),
    }
}

// ---------------------------------------------------------------------------
// ValueId Display helper (uses the existing Display impl from values.rs)
// ---------------------------------------------------------------------------

fn _fmt_value(v: ValueId) -> String {
    format!("{v}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::blocks::{BlockId, Terminator};
    use crate::tir::function::{TirFunction, TirModule};
    use crate::tir::ops::{AttrDict, AttrValue, Dialect, OpCode, TirOp};
    use crate::tir::types::TirType;
    use crate::tir::values::ValueId;

    fn make_add_function() -> TirFunction {
        // func @add(%0: i64, %1: i64) -> i64 {
        //   ^bb0(%0: i64, %1: i64):
        //     %2 = molt.add %0, %1
        //     return %2
        // }
        let mut func =
            TirFunction::new("add".into(), vec![TirType::I64, TirType::I64], TirType::I64);

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
    fn print_function_contains_func_name() {
        let func = make_add_function();
        let s = print_function(&func);
        assert!(s.contains("func @add"), "expected 'func @add' in:\n{}", s);
    }

    #[test]
    fn print_function_contains_opcode() {
        let func = make_add_function();
        let s = print_function(&func);
        assert!(s.contains("molt.add"), "expected 'molt.add' in:\n{}", s);
    }

    #[test]
    fn print_function_contains_return() {
        let func = make_add_function();
        let s = print_function(&func);
        assert!(s.contains("return"), "expected 'return' in:\n{}", s);
    }

    #[test]
    fn print_type_variants() {
        assert_eq!(print_type(&TirType::I64), "i64");
        assert_eq!(print_type(&TirType::F64), "f64");
        assert_eq!(print_type(&TirType::Bool), "bool");
        assert_eq!(print_type(&TirType::None), "none");
        assert_eq!(print_type(&TirType::Str), "str");
        assert_eq!(print_type(&TirType::DynBox), "dynbox");
        assert_eq!(print_type(&TirType::Never), "never");
        assert_eq!(
            print_type(&TirType::List(Box::new(TirType::I64))),
            "list<i64>"
        );
        assert_eq!(
            print_type(&TirType::Dict(
                Box::new(TirType::Str),
                Box::new(TirType::I64)
            )),
            "dict<str, i64>"
        );
    }

    #[test]
    fn print_opcode_spot_checks() {
        assert_eq!(print_opcode(&OpCode::Add), "add");
        assert_eq!(print_opcode(&OpCode::ConstInt), "const_int");
        assert_eq!(print_opcode(&OpCode::BuildList), "build_list");
        assert_eq!(print_opcode(&OpCode::Deopt), "deopt");
    }

    #[test]
    fn print_terminator_variants() {
        let ret = Terminator::Return {
            values: vec![ValueId(5)],
        };
        assert_eq!(print_terminator(&ret), "return %5");

        let br = Terminator::Branch {
            target: BlockId(1),
            args: vec![ValueId(3)],
        };
        let br_str = print_terminator(&br);
        assert!(br_str.contains("br"), "{}", br_str);
        assert!(br_str.contains("bb1"), "{}", br_str);

        let cond = Terminator::CondBranch {
            cond: ValueId(0),
            then_block: BlockId(1),
            then_args: vec![],
            else_block: BlockId(2),
            else_args: vec![],
        };
        let cond_str = print_terminator(&cond);
        assert!(cond_str.contains("cond_br"), "{}", cond_str);
        assert!(cond_str.contains("bb1"), "{}", cond_str);
        assert!(cond_str.contains("bb2"), "{}", cond_str);

        assert_eq!(print_terminator(&Terminator::Unreachable), "unreachable");
    }

    #[test]
    fn print_module_wraps_functions() {
        let func = make_add_function();
        let module = TirModule {
            name: "test_mod".into(),
            functions: vec![func],
            class_hierarchy: None,
        };
        let s = print_module(&module);
        assert!(s.contains("module @test_mod"), "{}", s);
        assert!(s.contains("func @add"), "{}", s);
    }

    #[test]
    fn print_op_with_attrs() {
        let op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ConstInt,
            operands: vec![],
            results: vec![ValueId(3)],
            attrs: {
                let mut m = AttrDict::new();
                m.insert("value".into(), AttrValue::Int(42));
                m
            },
            source_span: None,
        };
        let s = print_op(&op);
        assert!(s.contains("%3"), "{}", s);
        assert!(s.contains("const_int"), "{}", s);
        assert!(s.contains("42"), "{}", s);
    }
}
