//! SimpleIR → LuauIR lowering.
//!
//! Converts Molt's flat SSA-like IR into structured Luau AST nodes.
//! This enables optimization passes to operate on a proper AST instead
//! of on emitted source text.

use crate::ir::{FunctionIR, OpIR, SimpleIR};
use crate::luau_ir::*;
use std::collections::BTreeSet;

/// Lower a complete SimpleIR program to a LuauModule.
pub fn lower_to_luau(ir: &SimpleIR) -> LuauModule {
    let mut module = LuauModule {
        directives: vec!["--!native".to_string(), "--!strict".to_string()],
        prelude: vec![],
        functions: vec![],
        entry: vec![],
    };

    let emit_funcs: Vec<&FunctionIR> = ir
        .functions
        .iter()
        .filter(|f| !f.name.contains("__annotate__"))
        .collect();

    for func in &emit_funcs {
        module.functions.push(lower_function(func));
    }

    // Entry point
    module.entry.push(LuauStmt::If {
        cond: LuauExpr::Var("molt_main".to_string()),
        then_body: vec![LuauStmt::ExprStmt(LuauExpr::Call(
            Box::new(LuauExpr::Var("molt_main".to_string())),
            vec![],
        ))],
        elseif_chains: vec![],
        else_body: None,
    });

    module
}

/// Lower a single function.
fn lower_function(func: &FunctionIR) -> LuauFunction {
    let params: Vec<(String, LuauType)> = func
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ty = func
                .param_types
                .as_ref()
                .and_then(|pts| pts.get(i))
                .map(|t| python_type_to_luau_type(t))
                .unwrap_or(LuauType::Any);
            (sanitize_ident(p), ty)
        })
        .collect();

    let mut ctx = LowerCtx::new();
    let body = lower_ops(&func.ops, &mut ctx);

    LuauFunction {
        name: sanitize_ident(&func.name),
        params,
        return_type: None,
        body,
        is_native: true,
        is_local: true,
    }
}

/// Lowering context — tracks state during op conversion.
struct LowerCtx {
    /// Variables known to be lists (for type-directed optimization).
    list_vars: BTreeSet<String>,
    /// Variables known to be tuples (for multi-return unpack).
    #[allow(dead_code)]
    tuple_vars: BTreeSet<String>,
    /// Variables known to be numeric.
    numeric_vars: BTreeSet<String>,
}

impl LowerCtx {
    fn new() -> Self {
        Self {
            list_vars: BTreeSet::new(),
            tuple_vars: BTreeSet::new(),
            numeric_vars: BTreeSet::new(),
        }
    }
}

/// Lower a sequence of IR ops to Luau statements.
fn lower_ops(ops: &[OpIR], ctx: &mut LowerCtx) -> Vec<LuauStmt> {
    let mut stmts = Vec::new();
    let mut i = 0;

    while i < ops.len() {
        let op = &ops[i];
        match op.kind.as_str() {
            // =============================================================
            // Constants
            // =============================================================
            "const" => {
                let out = out_var(op);
                let expr = if let Some(v) = op.value {
                    LuauExpr::Lit(LuauLit::Int(v))
                } else if let Some(f) = op.f_value {
                    LuauExpr::Lit(LuauLit::Float(f))
                } else if let Some(ref s) = op.s_value {
                    LuauExpr::Lit(LuauLit::Str(s.clone()))
                } else {
                    LuauExpr::Lit(LuauLit::Nil)
                };
                stmts.push(LuauStmt::Local(out, None, expr));
            }
            "const_float" => {
                let out = out_var(op);
                let val = op.f_value.unwrap_or(0.0);
                ctx.numeric_vars.insert(out.clone());
                stmts.push(LuauStmt::Local(
                    out,
                    None,
                    LuauExpr::Lit(LuauLit::Float(val)),
                ));
            }
            "const_str" | "string_const" => {
                let out = out_var(op);
                let s = op.s_value.as_deref().unwrap_or("").to_string();
                stmts.push(LuauStmt::Local(
                    out,
                    None,
                    LuauExpr::Lit(LuauLit::Str(s)),
                ));
            }
            "const_bool" | "bool_const" => {
                let out = out_var(op);
                let val = op.value.unwrap_or(0) != 0;
                stmts.push(LuauStmt::Local(
                    out,
                    None,
                    LuauExpr::Lit(LuauLit::Bool(val)),
                ));
            }
            "const_none" | "none_const" => {
                let out = out_var(op);
                stmts.push(LuauStmt::Local(out, None, LuauExpr::Lit(LuauLit::Nil)));
            }

            // =============================================================
            // Arithmetic (with type tracking)
            // =============================================================
            "add" | "inplace_add" => {
                let out = out_var(op);
                let args = op_args(op);
                if args.len() >= 2 {
                    let lhs = var_expr(&args[0]);
                    let rhs = var_expr(&args[1]);
                    let is_numeric = op.fast_int == Some(true)
                        || op.fast_float == Some(true)
                        || matches!(op.type_hint.as_deref(), Some("int") | Some("float"))
                        || (ctx.numeric_vars.contains(&args[0])
                            && ctx.numeric_vars.contains(&args[1]));
                    let expr = if is_numeric {
                        ctx.numeric_vars.insert(out.clone());
                        LuauExpr::BinOp(Box::new(lhs), LuauBinOp::Add, Box::new(rhs))
                    } else {
                        // Type-checked: string concat or numeric add
                        LuauExpr::IfExpr(
                            Box::new(LuauExpr::BinOp(
                                Box::new(LuauExpr::Call(
                                    Box::new(LuauExpr::Var("type".to_string())),
                                    vec![var_expr(&args[0])],
                                )),
                                LuauBinOp::Eq,
                                Box::new(LuauExpr::Lit(LuauLit::Str("string".to_string()))),
                            )),
                            Box::new(LuauExpr::Concat(
                                Box::new(LuauExpr::Call(
                                    Box::new(LuauExpr::Var("tostring".to_string())),
                                    vec![var_expr(&args[0])],
                                )),
                                Box::new(LuauExpr::Call(
                                    Box::new(LuauExpr::Var("tostring".to_string())),
                                    vec![var_expr(&args[1])],
                                )),
                            )),
                            Box::new(LuauExpr::BinOp(
                                Box::new(lhs),
                                LuauBinOp::Add,
                                Box::new(rhs),
                            )),
                        )
                    };
                    stmts.push(LuauStmt::Local(out, None, expr));
                }
            }
            "sub" | "inplace_sub" => stmts.push(binary_op_stmt(op, LuauBinOp::Sub, ctx)),
            "mul" | "inplace_mul" => stmts.push(binary_op_stmt(op, LuauBinOp::Mul, ctx)),
            "div" => stmts.push(binary_op_stmt(op, LuauBinOp::Div, ctx)),
            "mod" => stmts.push(binary_op_stmt(op, LuauBinOp::Mod, ctx)),
            "pow" => stmts.push(binary_op_stmt(op, LuauBinOp::Pow, ctx)),
            "floordiv" => {
                // Luau // operator = LOP_IDIV opcode
                let out = out_var(op);
                let args = op_args(op);
                if args.len() >= 2 {
                    ctx.numeric_vars.insert(out.clone());
                    stmts.push(LuauStmt::Local(
                        out,
                        None,
                        LuauExpr::Raw(format!(
                            "{} // {}",
                            sanitize_ident(&args[0]),
                            sanitize_ident(&args[1])
                        )),
                    ));
                }
            }

            // =============================================================
            // Comparisons
            // =============================================================
            "lt" => stmts.push(binary_op_stmt(op, LuauBinOp::Lt, ctx)),
            "le" => stmts.push(binary_op_stmt(op, LuauBinOp::Le, ctx)),
            "gt" => stmts.push(binary_op_stmt(op, LuauBinOp::Gt, ctx)),
            "ge" => stmts.push(binary_op_stmt(op, LuauBinOp::Ge, ctx)),
            "eq" | "string_eq" | "is" => stmts.push(binary_op_stmt(op, LuauBinOp::Eq, ctx)),
            "ne" => stmts.push(binary_op_stmt(op, LuauBinOp::Ne, ctx)),
            "and" => stmts.push(binary_op_stmt(op, LuauBinOp::And, ctx)),
            "or" => stmts.push(binary_op_stmt(op, LuauBinOp::Or, ctx)),

            // =============================================================
            // Unary
            // =============================================================
            "not" => {
                let out = out_var(op);
                let args = op_args(op);
                if let Some(val) = args.first() {
                    stmts.push(LuauStmt::Local(
                        out,
                        None,
                        LuauExpr::UnOp(LuauUnOp::Not, Box::new(var_expr(val))),
                    ));
                }
            }

            // =============================================================
            // Control flow
            // =============================================================
            "if" => {
                let args = op_args(op);
                if let Some(cond) = args.first() {
                    // Collect then-body until else/end_if
                    let mut then_ops = Vec::new();
                    let mut else_ops = Vec::new();
                    let mut depth = 1i32;
                    let mut in_else = false;
                    let mut j = i + 1;
                    while j < ops.len() && depth > 0 {
                        match ops[j].kind.as_str() {
                            "if" => {
                                depth += 1;
                                if in_else {
                                    &mut else_ops
                                } else {
                                    &mut then_ops
                                }
                                .push(ops[j].clone());
                            }
                            "else" if depth == 1 => {
                                in_else = true;
                            }
                            "end_if" => {
                                depth -= 1;
                                if depth > 0 {
                                    if in_else {
                                        &mut else_ops
                                    } else {
                                        &mut then_ops
                                    }
                                    .push(ops[j].clone());
                                }
                            }
                            _ => {
                                if in_else {
                                    &mut else_ops
                                } else {
                                    &mut then_ops
                                }
                                .push(ops[j].clone());
                            }
                        }
                        j += 1;
                    }

                    let then_body = lower_ops(&then_ops, ctx);
                    let else_body = if else_ops.is_empty() {
                        None
                    } else {
                        Some(lower_ops(&else_ops, ctx))
                    };

                    stmts.push(LuauStmt::If {
                        cond: var_expr(cond),
                        then_body,
                        elseif_chains: vec![],
                        else_body,
                    });

                    i = j;
                    continue;
                }
            }
            "else" | "end_if" => {
                // Handled by the "if" arm above
            }

            // =============================================================
            // Loops
            // =============================================================
            "loop_start" => {
                // Collect loop body until loop_end
                let mut body_ops = Vec::new();
                let mut depth = 1i32;
                let mut j = i + 1;
                while j < ops.len() && depth > 0 {
                    match ops[j].kind.as_str() {
                        "loop_start" => {
                            depth += 1;
                            body_ops.push(ops[j].clone());
                        }
                        "loop_end" => {
                            depth -= 1;
                            if depth > 0 {
                                body_ops.push(ops[j].clone());
                            }
                        }
                        _ => body_ops.push(ops[j].clone()),
                    }
                    j += 1;
                }

                let body = lower_ops(&body_ops, ctx);
                stmts.push(LuauStmt::While(
                    LuauExpr::Lit(LuauLit::Bool(true)),
                    body,
                ));
                i = j;
                continue;
            }
            "loop_end" => {} // Handled by loop_start
            "loop_break" => stmts.push(LuauStmt::Break),
            "loop_break_if_true" => {
                let args = op_args(op);
                if let Some(cond) = args.first() {
                    stmts.push(LuauStmt::If {
                        cond: var_expr(cond),
                        then_body: vec![LuauStmt::Break],
                        elseif_chains: vec![],
                        else_body: None,
                    });
                }
            }
            "loop_break_if_false" => {
                let args = op_args(op);
                if let Some(cond) = args.first() {
                    stmts.push(LuauStmt::If {
                        cond: LuauExpr::UnOp(LuauUnOp::Not, Box::new(var_expr(cond))),
                        then_body: vec![LuauStmt::Break],
                        elseif_chains: vec![],
                        else_body: None,
                    });
                }
            }
            "loop_continue" => stmts.push(LuauStmt::Continue),

            "for_range" => {
                let out = out_var(op);
                let args = op_args(op);
                if args.len() >= 2 {
                    let start = var_expr(&args[0]);
                    let stop = LuauExpr::BinOp(
                        Box::new(var_expr(&args[1])),
                        LuauBinOp::Sub,
                        Box::new(LuauExpr::Lit(LuauLit::Int(1))),
                    );
                    let step = if args.len() >= 3 {
                        Some(var_expr(&args[2]))
                    } else {
                        None
                    };

                    // Collect body until end_for
                    let mut body_ops = Vec::new();
                    let mut depth = 1i32;
                    let mut j = i + 1;
                    while j < ops.len() && depth > 0 {
                        match ops[j].kind.as_str() {
                            "for_range" | "for_iter" => {
                                depth += 1;
                                body_ops.push(ops[j].clone());
                            }
                            "end_for" => {
                                depth -= 1;
                                if depth > 0 {
                                    body_ops.push(ops[j].clone());
                                }
                            }
                            _ => body_ops.push(ops[j].clone()),
                        }
                        j += 1;
                    }

                    let body = lower_ops(&body_ops, ctx);
                    ctx.numeric_vars.insert(out.clone());
                    stmts.push(LuauStmt::ForNumeric {
                        var: out,
                        start,
                        stop,
                        step,
                        body,
                    });
                    i = j;
                    continue;
                }
            }
            "for_iter" => {
                let out = out_var(op);
                let args = op_args(op);
                if let Some(iterable) = args.first() {
                    let mut body_ops = Vec::new();
                    let mut depth = 1i32;
                    let mut j = i + 1;
                    while j < ops.len() && depth > 0 {
                        match ops[j].kind.as_str() {
                            "for_range" | "for_iter" => {
                                depth += 1;
                                body_ops.push(ops[j].clone());
                            }
                            "end_for" => {
                                depth -= 1;
                                if depth > 0 {
                                    body_ops.push(ops[j].clone());
                                }
                            }
                            _ => body_ops.push(ops[j].clone()),
                        }
                        j += 1;
                    }

                    let body = lower_ops(&body_ops, ctx);
                    stmts.push(LuauStmt::ForGeneric {
                        vars: vec!["_".to_string(), out],
                        iter: LuauExpr::Call(
                            Box::new(LuauExpr::Var("ipairs".to_string())),
                            vec![var_expr(iterable)],
                        ),
                        body,
                    });
                    i = j;
                    continue;
                }
            }
            "end_for" => {} // Handled above

            // =============================================================
            // Return
            // =============================================================
            "ret" | "return" | "return_value" => {
                let exprs: Vec<LuauExpr> = if let Some(ref args) = op.args {
                    args.iter().map(|a| var_expr(a)).collect()
                } else if let Some(ref var) = op.var {
                    vec![var_expr(var)]
                } else {
                    vec![]
                };
                stmts.push(LuauStmt::Return(exprs));
            }
            "ret_void" => stmts.push(LuauStmt::Return(vec![])),

            // =============================================================
            // Collections
            // =============================================================
            "build_list" | "list_new" => {
                let out = out_var(op);
                let items: Vec<LuauTableEntry> = op_args(op)
                    .iter()
                    .map(|a| LuauTableEntry::Positional(var_expr(a)))
                    .collect();
                ctx.list_vars.insert(out.clone());
                stmts.push(LuauStmt::Local(out, None, LuauExpr::Table(items)));
            }
            "build_dict" | "dict_new" => {
                let out = out_var(op);
                let args = op_args(op);
                let entries: Vec<LuauTableEntry> = args
                    .chunks(2)
                    .filter(|c| c.len() == 2)
                    .map(|c| LuauTableEntry::Keyed(var_expr(&c[0]), var_expr(&c[1])))
                    .collect();
                stmts.push(LuauStmt::Local(out, None, LuauExpr::Table(entries)));
            }

            // =============================================================
            // List operations
            // =============================================================
            "list_append" => {
                let args = op_args(op);
                if args.len() >= 2 {
                    let list = var_expr(&args[0]);
                    let val = var_expr(&args[1]);
                    // list[#list + 1] = val (faster than table.insert)
                    stmts.push(LuauStmt::Assign(
                        LuauExpr::Index(
                            Box::new(list.clone()),
                            Box::new(LuauExpr::BinOp(
                                Box::new(LuauExpr::Len(Box::new(list))),
                                LuauBinOp::Add,
                                Box::new(LuauExpr::Lit(LuauLit::Int(1))),
                            )),
                        ),
                        val,
                    ));
                }
            }

            // =============================================================
            // Indexing (0-based -> 1-based)
            // =============================================================
            "get_item" | "subscript" | "index" => {
                let out = out_var(op);
                let args = op_args(op);
                if args.len() >= 2 {
                    let container = var_expr(&args[0]);
                    let key = var_expr(&args[1]);
                    let key_is_int = op.fast_int == Some(true)
                        || op.raw_int == Some(true)
                        || matches!(op.type_hint.as_deref(), Some("int"));
                    let index_expr = if key_is_int {
                        LuauExpr::Index(
                            Box::new(container),
                            Box::new(LuauExpr::BinOp(
                                Box::new(key),
                                LuauBinOp::Add,
                                Box::new(LuauExpr::Lit(LuauLit::Int(1))),
                            )),
                        )
                    } else {
                        LuauExpr::Index(
                            Box::new(container),
                            Box::new(LuauExpr::IfExpr(
                                Box::new(LuauExpr::BinOp(
                                    Box::new(LuauExpr::Call(
                                        Box::new(LuauExpr::Var("type".to_string())),
                                        vec![key.clone()],
                                    )),
                                    LuauBinOp::Eq,
                                    Box::new(LuauExpr::Lit(LuauLit::Str(
                                        "number".to_string(),
                                    ))),
                                )),
                                Box::new(LuauExpr::BinOp(
                                    Box::new(key.clone()),
                                    LuauBinOp::Add,
                                    Box::new(LuauExpr::Lit(LuauLit::Int(1))),
                                )),
                                Box::new(key),
                            )),
                        )
                    };
                    stmts.push(LuauStmt::Local(out, None, index_expr));
                }
            }

            // =============================================================
            // Print
            // =============================================================
            "print" => {
                let args: Vec<LuauExpr> = op_args(op).iter().map(|a| var_expr(a)).collect();
                stmts.push(LuauStmt::ExprStmt(LuauExpr::Call(
                    Box::new(LuauExpr::Var("molt_print".to_string())),
                    args,
                )));
            }

            // =============================================================
            // Labels and gotos
            // =============================================================
            "label" | "state_label" => {
                if let Some(id) = op.value {
                    stmts.push(LuauStmt::Label(format!("label_{id}")));
                }
            }
            "jump" | "goto" => {
                if let Some(id) = op.value {
                    stmts.push(LuauStmt::Goto(format!("label_{id}")));
                }
            }

            // =============================================================
            // Nops and internal ops
            // =============================================================
            "phi" | "nop" | "line" | "loop_carry_init" | "loop_carry_update" => {}

            // =============================================================
            // Everything else: emit as TODO comment for now
            // =============================================================
            _ => {
                stmts.push(LuauStmt::Comment(format!("[TODO: lower {}]", op.kind)));
            }
        }

        i += 1;
    }

    stmts
}

// ==========================================================================
// Helper functions
// ==========================================================================

fn out_var(op: &OpIR) -> String {
    op.out
        .as_deref()
        .map(sanitize_ident)
        .unwrap_or_else(|| "_".to_string())
}

fn op_args(op: &OpIR) -> Vec<String> {
    op.args.as_deref().unwrap_or(&[]).to_vec()
}

fn var_expr(name: &str) -> LuauExpr {
    LuauExpr::Var(sanitize_ident(name))
}

fn binary_op_stmt(op: &OpIR, bin_op: LuauBinOp, ctx: &mut LowerCtx) -> LuauStmt {
    let out = out_var(op);
    let args = op_args(op);
    if args.len() >= 2 {
        let lhs = var_expr(&args[0]);
        let rhs = var_expr(&args[1]);
        if matches!(
            bin_op,
            LuauBinOp::Add
                | LuauBinOp::Sub
                | LuauBinOp::Mul
                | LuauBinOp::Div
                | LuauBinOp::Mod
                | LuauBinOp::Pow
        ) {
            ctx.numeric_vars.insert(out.clone());
        }
        LuauStmt::Local(
            out,
            None,
            LuauExpr::BinOp(Box::new(lhs), bin_op, Box::new(rhs)),
        )
    } else {
        LuauStmt::Comment(format!("[binary op {} missing args]", op.kind))
    }
}

fn sanitize_ident(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '.' || c == '-' { '_' } else { c })
        .collect();
    if is_luau_keyword(&cleaned) {
        format!("_m_{cleaned}")
    } else if cleaned.starts_with(|c: char| c.is_ascii_digit()) {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

fn is_luau_keyword(word: &str) -> bool {
    matches!(
        word,
        "and"
            | "break"
            | "do"
            | "else"
            | "elseif"
            | "end"
            | "false"
            | "for"
            | "function"
            | "if"
            | "in"
            | "local"
            | "nil"
            | "not"
            | "or"
            | "repeat"
            | "return"
            | "then"
            | "true"
            | "until"
            | "while"
            | "continue"
            | "type"
            | "export"
    )
}

fn python_type_to_luau_type(hint: &str) -> LuauType {
    match hint {
        "int" | "Int" | "float" | "Float" => LuauType::Number,
        "str" | "Str" | "string" => LuauType::String,
        "bool" | "Bool" | "boolean" => LuauType::Boolean,
        "None" | "NoneType" => LuauType::Nil,
        "list" | "List" => LuauType::Table(Some(Box::new(LuauType::Any))),
        "dict" | "Dict" => LuauType::Dict(Box::new(LuauType::Any), Box::new(LuauType::Any)),
        _ => LuauType::Any,
    }
}

// ==========================================================================
// Tests
// ==========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;
    use crate::luau_ir::emit_luau;

    #[test]
    fn test_lower_simple_print() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                param_types: None,
                ops: vec![
                    OpIR {
                        kind: "const".to_string(),
                        out: Some("v0".to_string()),
                        value: Some(42),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "print".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };

        let module = lower_to_luau(&ir);
        let source = emit_luau(&module);
        assert!(source.contains("local v0 = 42"), "source: {source}");
        assert!(source.contains("molt_print(v0)"), "source: {source}");
    }

    #[test]
    fn test_lower_arithmetic() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec!["x".to_string()],
                param_types: Some(vec!["int".to_string()]),
                ops: vec![
                    OpIR {
                        kind: "add".to_string(),
                        out: Some("v0".to_string()),
                        args: Some(vec!["x".to_string(), "x".to_string()]),
                        fast_int: Some(true),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };

        let module = lower_to_luau(&ir);
        let source = emit_luau(&module);
        assert!(source.contains("x: number"), "source: {source}");
        assert!(source.contains("local v0 = x + x"), "source: {source}");
        assert!(source.contains("return v0"), "source: {source}");
    }

    #[test]
    fn test_lower_if_else() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                param_types: None,
                ops: vec![
                    OpIR {
                        kind: "const_bool".to_string(),
                        out: Some("v0".to_string()),
                        value: Some(1),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "if".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "print".to_string(),
                        args: Some(vec!["v0".to_string()]),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "else".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "ret_void".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "end_if".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };

        let module = lower_to_luau(&ir);
        let source = emit_luau(&module);
        assert!(source.contains("if v0 then"), "source: {source}");
        assert!(source.contains("molt_print(v0)"), "source: {source}");
        assert!(source.contains("else"), "source: {source}");
        assert!(source.contains("return"), "source: {source}");
    }

    #[test]
    fn test_lower_while_loop() {
        let ir = SimpleIR {
            functions: vec![FunctionIR {
                name: "molt_main".to_string(),
                params: vec![],
                param_types: None,
                ops: vec![
                    OpIR {
                        kind: "loop_start".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "loop_break".to_string(),
                        ..OpIR::default()
                    },
                    OpIR {
                        kind: "loop_end".to_string(),
                        ..OpIR::default()
                    },
                ],
            }],
            profile: None,
        };

        let module = lower_to_luau(&ir);
        let source = emit_luau(&module);
        assert!(source.contains("while true do"), "source: {source}");
        assert!(source.contains("break"), "source: {source}");
        assert!(source.contains("end"), "source: {source}");
    }
}
