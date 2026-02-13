use molt_obj_model::MoltObject;
use num_bigint::BigInt as NumBigInt;
use rustpython_parser::{Mode as ParseMode, ParseErrorType, ast as pyast, parse as parse_python};

use crate::{
    TYPE_ID_STRING, alloc_string, alloc_tuple, attr_name_bits_from_bytes, call_callable0,
    call_callable1, call_callable2, call_callable3, dec_ref_bits, decode_value_list, ellipsis_bits,
    exception_pending, inc_ref_bits, int_bits_from_bigint, missing_bits, molt_getattr_builtin,
    obj_from_bits, object_type_id, raise_exception, string_obj_to_owned,
};

struct AstParseCtors {
    module: u64,
    expression: u64,
    function_def: u64,
    arguments: u64,
    arg: u64,
    return_stmt: u64,
    expr_stmt: u64,
    name: u64,
    load: u64,
    constant: u64,
    add: u64,
    binop: u64,
    assign: u64,
    store: u64,
}

impl AstParseCtors {
    fn from_bits(_py: &crate::PyToken<'_>, ctors_bits: u64) -> Result<Self, u64> {
        let Some(values) = decode_value_list(obj_from_bits(ctors_bits)) else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "ast.parse constructor payload must be a tuple/list",
            ));
        };
        if values.len() != 14 {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "ast.parse constructor payload must include 14 constructors",
            ));
        }
        Ok(Self {
            module: values[0],
            expression: values[1],
            function_def: values[2],
            arguments: values[3],
            arg: values[4],
            return_stmt: values[5],
            expr_stmt: values[6],
            name: values[7],
            load: values[8],
            constant: values[9],
            add: values[10],
            binop: values[11],
            assign: values[12],
            store: values[13],
        })
    }
}

fn dec_if_heap(_py: &crate::PyToken<'_>, bits: u64) {
    if obj_from_bits(bits).as_ptr().is_some() {
        dec_ref_bits(_py, bits);
    }
}

fn alloc_string_bits(_py: &crate::PyToken<'_>, value: &str) -> Result<u64, u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        return Err(MoltObject::none().bits());
    }
    Ok(MoltObject::from_ptr(ptr).bits())
}

fn alloc_tuple_bits(_py: &crate::PyToken<'_>, elems: &[u64]) -> u64 {
    let ptr = alloc_tuple(_py, elems);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn call_ctor0(_py: &crate::PyToken<'_>, ctor_bits: u64) -> Result<u64, u64> {
    let out = unsafe { call_callable0(_py, ctor_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(out)
}

fn call_ctor1(_py: &crate::PyToken<'_>, ctor_bits: u64, arg0: u64) -> Result<u64, u64> {
    let out = unsafe { call_callable1(_py, ctor_bits, arg0) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(out)
}

fn call_ctor2(_py: &crate::PyToken<'_>, ctor_bits: u64, arg0: u64, arg1: u64) -> Result<u64, u64> {
    let out = unsafe { call_callable2(_py, ctor_bits, arg0, arg1) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(out)
}

fn call_ctor3(
    _py: &crate::PyToken<'_>,
    ctor_bits: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
) -> Result<u64, u64> {
    let out = unsafe { call_callable3(_py, ctor_bits, arg0, arg1, arg2) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(out)
}

fn parse_error_type(error: &ParseErrorType) -> &'static str {
    if error.is_tab_error() {
        "TabError"
    } else if error.is_indentation_error() {
        "IndentationError"
    } else {
        "SyntaxError"
    }
}

fn unsupported_expr(_py: &crate::PyToken<'_>, kind: &str) -> u64 {
    raise_exception::<_>(
        _py,
        "RuntimeError",
        &format!("molt ast.parse intrinsic unsupported expression node: {kind}"),
    )
}

fn unsupported_stmt(_py: &crate::PyToken<'_>, kind: &str) -> u64 {
    raise_exception::<_>(
        _py,
        "RuntimeError",
        &format!("molt ast.parse intrinsic unsupported statement node: {kind}"),
    )
}

fn convert_constant_value(_py: &crate::PyToken<'_>, value: &pyast::Constant) -> Result<u64, u64> {
    match value {
        pyast::Constant::None => Ok(MoltObject::none().bits()),
        pyast::Constant::Bool(v) => Ok(MoltObject::from_bool(*v).bits()),
        pyast::Constant::Str(v) => alloc_string_bits(_py, v),
        pyast::Constant::Bytes(v) => {
            let ptr = crate::alloc_bytes(_py, v);
            if ptr.is_null() {
                return Err(MoltObject::none().bits());
            }
            Ok(MoltObject::from_ptr(ptr).bits())
        }
        pyast::Constant::Int(v) => {
            let dec = v.to_string();
            let Some(parsed) = NumBigInt::parse_bytes(dec.as_bytes(), 10) else {
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "molt ast.parse intrinsic failed to decode integer constant",
                ));
            };
            Ok(int_bits_from_bigint(_py, parsed))
        }
        pyast::Constant::Tuple(items) => {
            let mut elem_bits: Vec<u64> = Vec::with_capacity(items.len());
            for item in items {
                let bits = match convert_constant_value(_py, item) {
                    Ok(bits) => bits,
                    Err(err) => {
                        for val in &elem_bits {
                            dec_if_heap(_py, *val);
                        }
                        return Err(err);
                    }
                };
                elem_bits.push(bits);
            }
            let out = alloc_tuple_bits(_py, &elem_bits);
            for val in &elem_bits {
                dec_if_heap(_py, *val);
            }
            if obj_from_bits(out).is_none() {
                return Err(MoltObject::none().bits());
            }
            Ok(out)
        }
        pyast::Constant::Float(v) => Ok(MoltObject::from_float(*v).bits()),
        pyast::Constant::Ellipsis => Ok(ellipsis_bits(_py)),
        pyast::Constant::Complex { .. } => Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "molt ast.parse intrinsic unsupported constant node: Complex",
        )),
    }
}

fn convert_name_with_ctx(
    _py: &crate::PyToken<'_>,
    name: &str,
    ctx_ctor: u64,
    ctors: &AstParseCtors,
) -> Result<u64, u64> {
    let id_bits = alloc_string_bits(_py, name)?;
    let ctx_bits = call_ctor0(_py, ctx_ctor)?;
    let out = call_ctor2(_py, ctors.name, id_bits, ctx_bits);
    dec_if_heap(_py, id_bits);
    dec_if_heap(_py, ctx_bits);
    out
}

fn convert_expr(
    _py: &crate::PyToken<'_>,
    expr: &pyast::Expr,
    ctors: &AstParseCtors,
) -> Result<u64, u64> {
    match expr {
        pyast::Expr::Constant(node) => {
            let value_bits = convert_constant_value(_py, &node.value)?;
            let out = call_ctor1(_py, ctors.constant, value_bits);
            dec_if_heap(_py, value_bits);
            out
        }
        pyast::Expr::Name(node) => convert_name_with_ctx(_py, node.id.as_str(), ctors.load, ctors),
        pyast::Expr::BinOp(node) => {
            let left_bits = convert_expr(_py, node.left.as_ref(), ctors)?;
            let right_bits = match convert_expr(_py, node.right.as_ref(), ctors) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_if_heap(_py, left_bits);
                    return Err(err);
                }
            };
            let op_bits = match node.op {
                pyast::Operator::Add => match call_ctor0(_py, ctors.add) {
                    Ok(bits) => bits,
                    Err(err) => {
                        dec_if_heap(_py, left_bits);
                        dec_if_heap(_py, right_bits);
                        return Err(err);
                    }
                },
                _ => {
                    dec_if_heap(_py, left_bits);
                    dec_if_heap(_py, right_bits);
                    return Err(unsupported_expr(_py, "BinOp(op!=Add)"));
                }
            };
            let out = call_ctor3(_py, ctors.binop, left_bits, op_bits, right_bits);
            dec_if_heap(_py, left_bits);
            dec_if_heap(_py, op_bits);
            dec_if_heap(_py, right_bits);
            out
        }
        _ => Err(unsupported_expr(_py, "unsupported")),
    }
}

fn convert_arg(
    _py: &crate::PyToken<'_>,
    arg: &pyast::ArgWithDefault,
    ctors: &AstParseCtors,
) -> Result<u64, u64> {
    if arg.default.is_some() {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "molt ast.parse intrinsic unsupported function arg defaults",
        ));
    }
    if arg.def.annotation.is_some() || arg.def.type_comment.is_some() {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "molt ast.parse intrinsic unsupported function arg annotations",
        ));
    }
    let name_bits = alloc_string_bits(_py, arg.def.arg.as_str())?;
    let out = call_ctor1(_py, ctors.arg, name_bits);
    dec_if_heap(_py, name_bits);
    out
}

fn convert_assign_target(
    _py: &crate::PyToken<'_>,
    target: &pyast::Expr,
    ctors: &AstParseCtors,
) -> Result<u64, u64> {
    match target {
        pyast::Expr::Name(node) => convert_name_with_ctx(_py, node.id.as_str(), ctors.store, ctors),
        _ => Err(unsupported_stmt(_py, "Assign(target!=Name)")),
    }
}

fn convert_stmt(
    _py: &crate::PyToken<'_>,
    stmt: &pyast::Stmt,
    ctors: &AstParseCtors,
) -> Result<u64, u64> {
    match stmt {
        pyast::Stmt::FunctionDef(node) => {
            if !node.decorator_list.is_empty()
                || node.returns.is_some()
                || node.type_comment.is_some()
                || !node.type_params.is_empty()
            {
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "molt ast.parse intrinsic unsupported FunctionDef metadata",
                ));
            }
            if !node.args.posonlyargs.is_empty()
                || node.args.vararg.is_some()
                || !node.args.kwonlyargs.is_empty()
                || node.args.kwarg.is_some()
            {
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "molt ast.parse intrinsic unsupported function signature shape",
                ));
            }

            let name_bits = alloc_string_bits(_py, node.name.as_str())?;

            let mut arg_nodes: Vec<u64> = Vec::with_capacity(node.args.args.len());
            for arg in &node.args.args {
                let arg_bits = match convert_arg(_py, arg, ctors) {
                    Ok(bits) => bits,
                    Err(err) => {
                        dec_if_heap(_py, name_bits);
                        for bits in &arg_nodes {
                            dec_if_heap(_py, *bits);
                        }
                        return Err(err);
                    }
                };
                arg_nodes.push(arg_bits);
            }
            let arg_tuple_bits = alloc_tuple_bits(_py, &arg_nodes);
            for bits in &arg_nodes {
                dec_if_heap(_py, *bits);
            }
            if obj_from_bits(arg_tuple_bits).is_none() {
                dec_if_heap(_py, name_bits);
                return Err(MoltObject::none().bits());
            }
            let args_obj_bits = match call_ctor1(_py, ctors.arguments, arg_tuple_bits) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_if_heap(_py, name_bits);
                    dec_if_heap(_py, arg_tuple_bits);
                    return Err(err);
                }
            };
            dec_if_heap(_py, arg_tuple_bits);

            let mut body_nodes: Vec<u64> = Vec::with_capacity(node.body.len());
            for child in &node.body {
                let child_bits = match convert_stmt(_py, child, ctors) {
                    Ok(bits) => bits,
                    Err(err) => {
                        dec_if_heap(_py, name_bits);
                        dec_if_heap(_py, args_obj_bits);
                        for bits in &body_nodes {
                            dec_if_heap(_py, *bits);
                        }
                        return Err(err);
                    }
                };
                body_nodes.push(child_bits);
            }
            let body_bits = alloc_tuple_bits(_py, &body_nodes);
            for bits in &body_nodes {
                dec_if_heap(_py, *bits);
            }
            if obj_from_bits(body_bits).is_none() {
                dec_if_heap(_py, name_bits);
                dec_if_heap(_py, args_obj_bits);
                return Err(MoltObject::none().bits());
            }

            let out = call_ctor3(_py, ctors.function_def, name_bits, args_obj_bits, body_bits);
            dec_if_heap(_py, name_bits);
            dec_if_heap(_py, args_obj_bits);
            dec_if_heap(_py, body_bits);
            out
        }
        pyast::Stmt::Assign(node) => {
            if node.type_comment.is_some() {
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "molt ast.parse intrinsic unsupported Assign type_comment",
                ));
            }
            let mut target_nodes: Vec<u64> = Vec::with_capacity(node.targets.len());
            for target in &node.targets {
                let target_bits = match convert_assign_target(_py, target, ctors) {
                    Ok(bits) => bits,
                    Err(err) => {
                        for bits in &target_nodes {
                            dec_if_heap(_py, *bits);
                        }
                        return Err(err);
                    }
                };
                target_nodes.push(target_bits);
            }
            let targets_bits = alloc_tuple_bits(_py, &target_nodes);
            for bits in &target_nodes {
                dec_if_heap(_py, *bits);
            }
            if obj_from_bits(targets_bits).is_none() {
                return Err(MoltObject::none().bits());
            }
            let value_bits = match convert_expr(_py, node.value.as_ref(), ctors) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_if_heap(_py, targets_bits);
                    return Err(err);
                }
            };
            let out = call_ctor2(_py, ctors.assign, targets_bits, value_bits);
            dec_if_heap(_py, targets_bits);
            dec_if_heap(_py, value_bits);
            out
        }
        pyast::Stmt::Return(node) => {
            let value_bits = if let Some(value) = node.value.as_ref() {
                convert_expr(_py, value.as_ref(), ctors)?
            } else {
                MoltObject::none().bits()
            };
            let out = call_ctor1(_py, ctors.return_stmt, value_bits);
            dec_if_heap(_py, value_bits);
            out
        }
        pyast::Stmt::Expr(node) => {
            let value_bits = convert_expr(_py, &node.value, ctors)?;
            let out = call_ctor1(_py, ctors.expr_stmt, value_bits);
            dec_if_heap(_py, value_bits);
            out
        }
        _ => Err(unsupported_stmt(_py, "unsupported")),
    }
}

fn get_attr_optional(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
) -> Result<Option<u64>, u64> {
    let Some(name_bits) = attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(obj_bits, name_bits, missing);
    dec_ref_bits(_py, name_bits);
    if exception_pending(_py) {
        if crate::builtins::attr::clear_attribute_error_if_pending(_py) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if value_bits == missing {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

fn node_kind_name(_py: &crate::PyToken<'_>, obj_bits: u64) -> Result<Option<String>, u64> {
    let Some(class_bits) = get_attr_optional(_py, obj_bits, b"__class__")? else {
        return Ok(None);
    };
    let out = match get_attr_optional(_py, class_bits, b"__name__")? {
        Some(name_bits) => {
            let name = string_obj_to_owned(obj_from_bits(name_bits));
            dec_if_heap(_py, name_bits);
            name
        }
        None => None,
    };
    dec_if_heap(_py, class_bits);
    Ok(out)
}

fn push_attr_child(
    _py: &crate::PyToken<'_>,
    node_bits: u64,
    name: &[u8],
    children: &mut Vec<u64>,
) -> Result<(), u64> {
    let Some(value_bits) = get_attr_optional(_py, node_bits, name)? else {
        return Ok(());
    };
    if !obj_from_bits(value_bits).is_none() {
        inc_ref_bits(_py, value_bits);
        children.push(value_bits);
    }
    dec_if_heap(_py, value_bits);
    Ok(())
}

fn push_attr_children_from_seq(
    _py: &crate::PyToken<'_>,
    node_bits: u64,
    name: &[u8],
    children: &mut Vec<u64>,
) -> Result<(), u64> {
    let Some(seq_bits) = get_attr_optional(_py, node_bits, name)? else {
        return Ok(());
    };
    if let Some(items) = decode_value_list(obj_from_bits(seq_bits)) {
        for item_bits in items {
            if obj_from_bits(item_bits).is_none() {
                continue;
            }
            inc_ref_bits(_py, item_bits);
            children.push(item_bits);
        }
    }
    dec_if_heap(_py, seq_bits);
    Ok(())
}

fn collect_child_nodes(_py: &crate::PyToken<'_>, node_bits: u64) -> Result<Vec<u64>, u64> {
    let mut children: Vec<u64> = Vec::new();
    let kind = node_kind_name(_py, node_bits)?;
    match kind.as_deref() {
        Some("Module") => push_attr_children_from_seq(_py, node_bits, b"body", &mut children)?,
        Some("Expression") => push_attr_child(_py, node_bits, b"body", &mut children)?,
        Some("FunctionDef") => {
            push_attr_child(_py, node_bits, b"args", &mut children)?;
            push_attr_children_from_seq(_py, node_bits, b"body", &mut children)?;
        }
        Some("arguments") => push_attr_children_from_seq(_py, node_bits, b"args", &mut children)?,
        Some("Return") => push_attr_child(_py, node_bits, b"value", &mut children)?,
        Some("Expr") => push_attr_child(_py, node_bits, b"value", &mut children)?,
        Some("Assign") => {
            push_attr_children_from_seq(_py, node_bits, b"targets", &mut children)?;
            push_attr_child(_py, node_bits, b"value", &mut children)?;
        }
        Some("BinOp") => {
            push_attr_child(_py, node_bits, b"left", &mut children)?;
            push_attr_child(_py, node_bits, b"op", &mut children)?;
            push_attr_child(_py, node_bits, b"right", &mut children)?;
        }
        Some("Name") => push_attr_child(_py, node_bits, b"ctx", &mut children)?,
        _ => {}
    }
    Ok(children)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ast_parse(
    source_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    type_comments_bits: u64,
    feature_version_bits: u64,
    ctors_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "source must be str"),
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "filename must be str"),
        };
        let mode = match string_obj_to_owned(obj_from_bits(mode_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "mode must be str"),
        };
        let _ = type_comments_bits;
        let _ = feature_version_bits;
        let ctors = match AstParseCtors::from_bits(_py, ctors_bits) {
            Ok(ctors) => ctors,
            Err(err) => return err,
        };

        let parse_mode = match mode.as_str() {
            "exec" => ParseMode::Module,
            "eval" => ParseMode::Expression,
            _ => return raise_exception::<_>(_py, "ValueError", "mode must be 'exec' or 'eval'"),
        };
        let parsed = match parse_python(&source, parse_mode, &filename) {
            Ok(value) => value,
            Err(err) => {
                let typ = parse_error_type(&err.error);
                return raise_exception::<_>(_py, typ, &err.error.to_string());
            }
        };

        // TODO(stdlib-compat, owner:stdlib, milestone:SL3, priority:P2, status:partial):
        // extend Rust ast lowering to additional stmt/expr variants and full argument
        // shape parity; unsupported nodes currently raise RuntimeError immediately.
        match parsed {
            pyast::Mod::Module(module) => {
                let mut body_nodes: Vec<u64> = Vec::with_capacity(module.body.len());
                for stmt in &module.body {
                    let stmt_bits = match convert_stmt(_py, stmt, &ctors) {
                        Ok(bits) => bits,
                        Err(err) => {
                            for bits in &body_nodes {
                                dec_if_heap(_py, *bits);
                            }
                            return err;
                        }
                    };
                    body_nodes.push(stmt_bits);
                }
                let body_bits = alloc_tuple_bits(_py, &body_nodes);
                for bits in &body_nodes {
                    dec_if_heap(_py, *bits);
                }
                if obj_from_bits(body_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let out = call_ctor1(_py, ctors.module, body_bits);
                dec_if_heap(_py, body_bits);
                match out {
                    Ok(bits) => bits,
                    Err(err) => err,
                }
            }
            pyast::Mod::Expression(expr) => {
                let body_bits = match convert_expr(_py, expr.body.as_ref(), &ctors) {
                    Ok(bits) => bits,
                    Err(err) => return err,
                };
                let out = call_ctor1(_py, ctors.expression, body_bits);
                dec_if_heap(_py, body_bits);
                match out {
                    Ok(bits) => bits,
                    Err(err) => err,
                }
            }
            _ => raise_exception::<_>(
                _py,
                "RuntimeError",
                "molt ast.parse intrinsic unsupported parse mode result",
            ),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ast_walk(node_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let kind = match node_kind_name(_py, node_bits) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let Some(kind) = kind else {
            return raise_exception::<_>(_py, "TypeError", "ast.walk() expected AST node");
        };
        if !matches!(
            kind.as_str(),
            "Module"
                | "Expression"
                | "FunctionDef"
                | "arguments"
                | "Return"
                | "Expr"
                | "Name"
                | "Load"
                | "Constant"
                | "Add"
                | "BinOp"
        ) {
            return raise_exception::<_>(_py, "TypeError", "ast.walk() expected AST node");
        }
        let mut stack: Vec<u64> = Vec::new();
        inc_ref_bits(_py, node_bits);
        stack.push(node_bits);
        let mut out: Vec<u64> = Vec::new();

        while let Some(current_bits) = stack.pop() {
            let children = match collect_child_nodes(_py, current_bits) {
                Ok(children) => children,
                Err(err) => {
                    dec_if_heap(_py, current_bits);
                    for bits in &stack {
                        dec_if_heap(_py, *bits);
                    }
                    for bits in &out {
                        dec_if_heap(_py, *bits);
                    }
                    return err;
                }
            };
            for child in children {
                stack.push(child);
            }
            out.push(current_bits);
        }

        let out_bits = alloc_tuple_bits(_py, &out);
        for bits in &out {
            dec_if_heap(_py, *bits);
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ast_get_docstring(node_bits: u64, clean_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let _ = clean_bits;
        let Some(body_bits) = (match get_attr_optional(_py, node_bits, b"body") {
            Ok(value) => value,
            Err(err) => return err,
        }) else {
            return MoltObject::none().bits();
        };
        let Some(body_items) = decode_value_list(obj_from_bits(body_bits)) else {
            dec_if_heap(_py, body_bits);
            return MoltObject::none().bits();
        };
        if body_items.is_empty() {
            dec_if_heap(_py, body_bits);
            return MoltObject::none().bits();
        }
        let first_bits = body_items[0];
        let Some(expr_value_bits) = (match get_attr_optional(_py, first_bits, b"value") {
            Ok(value) => value,
            Err(err) => {
                dec_if_heap(_py, body_bits);
                return err;
            }
        }) else {
            dec_if_heap(_py, body_bits);
            return MoltObject::none().bits();
        };
        let Some(const_value_bits) = (match get_attr_optional(_py, expr_value_bits, b"value") {
            Ok(value) => value,
            Err(err) => {
                dec_if_heap(_py, expr_value_bits);
                dec_if_heap(_py, body_bits);
                return err;
            }
        }) else {
            dec_if_heap(_py, expr_value_bits);
            dec_if_heap(_py, body_bits);
            return MoltObject::none().bits();
        };

        let out = if let Some(ptr) = obj_from_bits(const_value_bits).as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_STRING {
                    const_value_bits
                } else {
                    dec_if_heap(_py, const_value_bits);
                    MoltObject::none().bits()
                }
            }
        } else {
            MoltObject::none().bits()
        };
        dec_if_heap(_py, expr_value_bits);
        dec_if_heap(_py, body_bits);
        out
    })
}
