use super::*;

#[cfg(feature = "stdlib_ast")]
use rustpython_parser::{Mode as ParseMode, ParseErrorType, ast as pyast, parse as parse_python};
#[cfg(feature = "stdlib_ast")]
use std::collections::HashSet;

// --- Begin stdlib_ast-gated compile infrastructure ---
#[cfg(feature = "stdlib_ast")]
fn compile_error_type(error: &ParseErrorType) -> &'static str {
    if error.is_tab_error() {
        "TabError"
    } else if error.is_indentation_error() {
        "IndentationError"
    } else {
        "SyntaxError"
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_future_flag_for_name(name: &str) -> i64 {
    match name {
        "nested_scopes" => 0x0010,
        "generators" => 0,
        "division" => 0x20000,
        "absolute_import" => 0x40000,
        "with_statement" => 0x80000,
        "print_function" => 0x100000,
        "unicode_literals" => 0x200000,
        "barry_as_FLUFL" => 0x400000,
        "generator_stop" => 0x800000,
        "annotations" => 0x1000000,
        _ => 0,
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_is_docstring_stmt(stmt: &pyast::Stmt) -> bool {
    match stmt {
        pyast::Stmt::Expr(node) => match node.value.as_ref() {
            pyast::Expr::Constant(expr) => matches!(expr.value, pyast::Constant::Str(_)),
            _ => false,
        },
        _ => false,
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_future_flags_from_stmts(stmts: &[pyast::Stmt]) -> i64 {
    let mut idx = 0usize;
    if let Some(first) = stmts.first()
        && codeop_is_docstring_stmt(first)
    {
        idx = 1;
    }
    let mut out = 0i64;
    for stmt in &stmts[idx..] {
        let pyast::Stmt::ImportFrom(node) = stmt else {
            break;
        };
        let Some(module) = node.module.as_ref() else {
            break;
        };
        let level_is_zero = match node.level.as_ref() {
            None => true,
            Some(value) => value.to_u32() == 0,
        };
        if module.as_str() != "__future__" || !level_is_zero {
            break;
        }
        for alias in &node.names {
            out |= codeop_future_flag_for_name(alias.name.as_str());
        }
    }
    out
}

#[cfg(feature = "stdlib_ast")]
fn codeop_future_flags_from_parsed(parsed: &pyast::Mod) -> i64 {
    match parsed {
        pyast::Mod::Module(module) => codeop_future_flags_from_stmts(&module.body),
        pyast::Mod::Interactive(module) => codeop_future_flags_from_stmts(&module.body),
        _ => 0,
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeop_stmt_is_compound(stmt: &pyast::Stmt) -> bool {
    matches!(
        stmt,
        pyast::Stmt::FunctionDef(_)
            | pyast::Stmt::AsyncFunctionDef(_)
            | pyast::Stmt::ClassDef(_)
            | pyast::Stmt::If(_)
            | pyast::Stmt::For(_)
            | pyast::Stmt::AsyncFor(_)
            | pyast::Stmt::While(_)
            | pyast::Stmt::With(_)
            | pyast::Stmt::AsyncWith(_)
            | pyast::Stmt::Try(_)
            | pyast::Stmt::TryStar(_)
            | pyast::Stmt::Match(_)
    )
}

#[cfg(feature = "stdlib_ast")]
fn codeop_source_incomplete_after_success(source: &str, mode: &str, parsed: &pyast::Mod) -> bool {
    if mode != "single" {
        return false;
    }
    if source.trim_end().ends_with(':') {
        return true;
    }
    if source.contains('\n')
        && !source.ends_with('\n')
        && let pyast::Mod::Interactive(module) = parsed
        && let Some(first) = module.body.first()
    {
        return codeop_stmt_is_compound(first);
    }
    false
}

#[cfg(feature = "stdlib_ast")]
fn codeop_source_has_missing_indented_suite(source: &str) -> bool {
    let lines: Vec<&str> = source.split('\n').collect();
    let leading_indent = |line: &str| -> usize {
        line.chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .count()
    };

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !trimmed.ends_with(':') {
            continue;
        }
        let indent = leading_indent(line);
        let mut next_idx = idx + 1;
        while next_idx < lines.len() {
            let next_line = lines[next_idx];
            let next_trimmed = next_line.trim();
            if next_trimmed.is_empty() || next_trimmed.starts_with('#') {
                next_idx += 1;
                continue;
            }
            if leading_indent(next_line) <= indent {
                return true;
            }
            break;
        }
    }
    false
}

#[cfg(feature = "stdlib_ast")]
fn codeop_parse_error_is_incomplete(error: &ParseErrorType, source: &str) -> bool {
    let trimmed = source.trim_end();
    let trailing_backslash_newline = source.ends_with("\\\n") || source.ends_with("\\\r\n");
    match error {
        ParseErrorType::Eof => !trailing_backslash_newline,
        ParseErrorType::UnrecognizedToken(_, expected) => expected.as_deref() == Some("Indent"),
        ParseErrorType::Lexical(lex) => {
            let text = lex.to_string();
            if text.contains("unexpected EOF") {
                return true;
            }
            if text.contains("line continuation") {
                return !trailing_backslash_newline;
            }
            if text.contains("unexpected string") {
                return true;
            }
            (text.contains("expected an indented block")
                || text.contains("unindent does not match any outer indentation level"))
                && trimmed.ends_with(':')
        }
        _ => false,
    }
}

#[cfg(feature = "stdlib_ast")]
enum CodeopCompileStatus {
    Compiled {
        next_flags: i64,
    },
    Incomplete,
    Error {
        error_type: &'static str,
        message: String,
    },
}

#[cfg(feature = "stdlib_ast")]
fn codeop_compile_status(
    source: &str,
    filename: &str,
    mode: &str,
    flags: i64,
    incomplete_input: bool,
) -> CodeopCompileStatus {
    let parse_mode = match mode {
        "exec" => ParseMode::Module,
        "eval" => ParseMode::Expression,
        "single" => ParseMode::Interactive,
        _ => {
            return CodeopCompileStatus::Error {
                error_type: "ValueError",
                message: "compile() mode must be 'exec', 'eval' or 'single'".to_string(),
            };
        }
    };
    match parse_python(source, parse_mode, filename) {
        Ok(parsed) => match compile_validate_nonlocal_semantics(&parsed) {
            Ok(()) => {
                if codeop_source_has_missing_indented_suite(source) {
                    return CodeopCompileStatus::Error {
                        error_type: "SyntaxError",
                        message: "expected an indented block".to_string(),
                    };
                }
                if incomplete_input && codeop_source_incomplete_after_success(source, mode, &parsed)
                {
                    return CodeopCompileStatus::Incomplete;
                }
                CodeopCompileStatus::Compiled {
                    next_flags: flags | codeop_future_flags_from_parsed(&parsed),
                }
            }
            Err(message) => CodeopCompileStatus::Error {
                error_type: "SyntaxError",
                message,
            },
        },
        Err(err) => {
            if incomplete_input && codeop_parse_error_is_incomplete(&err.error, source) {
                CodeopCompileStatus::Incomplete
            } else {
                CodeopCompileStatus::Error {
                    error_type: compile_error_type(&err.error),
                    message: err.error.to_string(),
                }
            }
        }
    }
}

#[cfg(feature = "stdlib_ast")]
fn codeobj_from_filename_bits(_py: &crate::PyToken<'_>, filename_bits: u64) -> u64 {
    let name_ptr = alloc_string(_py, b"<module>");
    if name_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let varnames_ptr = alloc_tuple(_py, &[]);
    if varnames_ptr.is_null() {
        dec_ref_bits(_py, name_bits);
        return MoltObject::none().bits();
    }
    let varnames_bits = MoltObject::from_ptr(varnames_ptr).bits();
    let names_ptr = alloc_tuple(_py, &[]);
    if names_ptr.is_null() {
        dec_ref_bits(_py, varnames_bits);
        dec_ref_bits(_py, name_bits);
        return MoltObject::none().bits();
    }
    let names_bits = MoltObject::from_ptr(names_ptr).bits();
    let code_ptr = alloc_code_obj(
        _py,
        filename_bits,
        name_bits,
        1,
        MoltObject::none().bits(),
        varnames_bits,
        names_bits,
        0,
        0,
        0,
    );
    dec_ref_bits(_py, names_bits);
    dec_ref_bits(_py, varnames_bits);
    dec_ref_bits(_py, name_bits);
    if code_ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(code_ptr).bits()
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_bound_names_in_target(target: &pyast::Expr, out: &mut HashSet<String>) {
    match target {
        pyast::Expr::Name(node) => {
            out.insert(node.id.as_str().to_string());
        }
        pyast::Expr::Tuple(node) => {
            for elt in &node.elts {
                collect_bound_names_in_target(elt, out);
            }
        }
        pyast::Expr::List(node) => {
            for elt in &node.elts {
                collect_bound_names_in_target(elt, out);
            }
        }
        pyast::Expr::Starred(node) => {
            collect_bound_names_in_target(node.value.as_ref(), out);
        }
        _ => {}
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_import_binding(alias: &pyast::Alias, out: &mut HashSet<String>) {
    if let Some(asname) = alias.asname.as_ref() {
        out.insert(asname.as_str().to_string());
        return;
    }
    let raw = alias.name.as_str();
    let base = raw.split('.').next().unwrap_or(raw);
    if !base.is_empty() {
        out.insert(base.to_string());
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_arg_bindings(args: &pyast::Arguments, out: &mut HashSet<String>) {
    for arg in &args.posonlyargs {
        out.insert(arg.def.arg.as_str().to_string());
    }
    for arg in &args.args {
        out.insert(arg.def.arg.as_str().to_string());
    }
    if let Some(vararg) = args.vararg.as_ref() {
        out.insert(vararg.arg.as_str().to_string());
    }
    for arg in &args.kwonlyargs {
        out.insert(arg.def.arg.as_str().to_string());
    }
    if let Some(kwarg) = args.kwarg.as_ref() {
        out.insert(kwarg.arg.as_str().to_string());
    }
}

#[cfg(feature = "stdlib_ast")]
fn collect_function_scope_info(
    stmt: &pyast::Stmt,
    local_bindings: &mut HashSet<String>,
    nonlocal_decls: &mut HashSet<String>,
    global_decls: &mut HashSet<String>,
) {
    match stmt {
        pyast::Stmt::FunctionDef(node) => {
            local_bindings.insert(node.name.as_str().to_string());
        }
        pyast::Stmt::AsyncFunctionDef(node) => {
            local_bindings.insert(node.name.as_str().to_string());
        }
        pyast::Stmt::ClassDef(node) => {
            local_bindings.insert(node.name.as_str().to_string());
        }
        pyast::Stmt::Global(node) => {
            for name in &node.names {
                global_decls.insert(name.as_str().to_string());
            }
        }
        pyast::Stmt::Nonlocal(node) => {
            for name in &node.names {
                nonlocal_decls.insert(name.as_str().to_string());
            }
        }
        pyast::Stmt::Assign(node) => {
            for target in &node.targets {
                collect_bound_names_in_target(target, local_bindings);
            }
        }
        pyast::Stmt::AnnAssign(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
        }
        pyast::Stmt::AugAssign(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
        }
        pyast::Stmt::For(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::AsyncFor(node) => {
            collect_bound_names_in_target(node.target.as_ref(), local_bindings);
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::With(node) => {
            for item in &node.items {
                if let Some(target) = item.optional_vars.as_ref() {
                    collect_bound_names_in_target(target.as_ref(), local_bindings);
                }
            }
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::AsyncWith(node) => {
            for item in &node.items {
                if let Some(target) = item.optional_vars.as_ref() {
                    collect_bound_names_in_target(target.as_ref(), local_bindings);
                }
            }
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::If(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::While(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::Try(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                if let Some(name) = handler.name.as_ref() {
                    local_bindings.insert(name.as_str().to_string());
                }
                for child in &handler.body {
                    collect_function_scope_info(
                        child,
                        local_bindings,
                        nonlocal_decls,
                        global_decls,
                    );
                }
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.finalbody {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::TryStar(node) => {
            for child in &node.body {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                if let Some(name) = handler.name.as_ref() {
                    local_bindings.insert(name.as_str().to_string());
                }
                for child in &handler.body {
                    collect_function_scope_info(
                        child,
                        local_bindings,
                        nonlocal_decls,
                        global_decls,
                    );
                }
            }
            for child in &node.orelse {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
            for child in &node.finalbody {
                collect_function_scope_info(child, local_bindings, nonlocal_decls, global_decls);
            }
        }
        pyast::Stmt::Match(node) => {
            for case in &node.cases {
                for child in &case.body {
                    collect_function_scope_info(
                        child,
                        local_bindings,
                        nonlocal_decls,
                        global_decls,
                    );
                }
            }
        }
        pyast::Stmt::Import(node) => {
            for alias in &node.names {
                collect_import_binding(alias, local_bindings);
            }
        }
        pyast::Stmt::ImportFrom(node) => {
            for alias in &node.names {
                collect_import_binding(alias, local_bindings);
            }
        }
        _ => {}
    }
}

#[cfg(feature = "stdlib_ast")]
fn walk_nested_function_scopes(
    stmts: &[pyast::Stmt],
    enclosing_function_bindings: &[HashSet<String>],
) -> Result<(), String> {
    for stmt in stmts {
        match stmt {
            pyast::Stmt::FunctionDef(node) => {
                validate_function_scope(&node.args, &node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::AsyncFunctionDef(node) => {
                validate_function_scope(&node.args, &node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::ClassDef(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::If(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::For(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::AsyncFor(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::While(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
            }
            pyast::Stmt::With(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::AsyncWith(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
            }
            pyast::Stmt::Try(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                for handler in &node.handlers {
                    let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                    walk_nested_function_scopes(&handler.body, enclosing_function_bindings)?;
                }
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.finalbody, enclosing_function_bindings)?;
            }
            pyast::Stmt::TryStar(node) => {
                walk_nested_function_scopes(&node.body, enclosing_function_bindings)?;
                for handler in &node.handlers {
                    let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                    walk_nested_function_scopes(&handler.body, enclosing_function_bindings)?;
                }
                walk_nested_function_scopes(&node.orelse, enclosing_function_bindings)?;
                walk_nested_function_scopes(&node.finalbody, enclosing_function_bindings)?;
            }
            pyast::Stmt::Match(node) => {
                for case in &node.cases {
                    walk_nested_function_scopes(&case.body, enclosing_function_bindings)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(feature = "stdlib_ast")]
fn validate_control_flow_stmt(
    stmt: &pyast::Stmt,
    in_function: bool,
    in_loop: bool,
) -> Result<(), String> {
    fn validate_delete_target(target: &pyast::Expr) -> Result<(), String> {
        match target {
            pyast::Expr::Name(_) | pyast::Expr::Attribute(_) | pyast::Expr::Subscript(_) => Ok(()),
            pyast::Expr::Tuple(node) => {
                for elt in &node.elts {
                    validate_delete_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::List(node) => {
                for elt in &node.elts {
                    validate_delete_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::Constant(_) => Err("cannot delete literal".to_string()),
            _ => Err("cannot delete expression".to_string()),
        }
    }

    fn validate_assign_target(target: &pyast::Expr) -> Result<(), String> {
        match target {
            pyast::Expr::ListComp(_) => Err(
                "cannot assign to list comprehension here. Maybe you meant '==' instead of '='?"
                    .to_string(),
            ),
            pyast::Expr::Tuple(node) => {
                for elt in &node.elts {
                    validate_assign_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::List(node) => {
                for elt in &node.elts {
                    validate_assign_target(elt)?;
                }
                Ok(())
            }
            pyast::Expr::Starred(node) => validate_assign_target(node.value.as_ref()),
            _ => Ok(()),
        }
    }

    match stmt {
        pyast::Stmt::Return(_) => {
            if !in_function {
                return Err("'return' outside function".to_string());
            }
        }
        pyast::Stmt::Break(_) => {
            if !in_loop {
                return Err("'break' outside loop".to_string());
            }
        }
        pyast::Stmt::Continue(_) => {
            if !in_loop {
                return Err("'continue' not properly in loop".to_string());
            }
        }
        pyast::Stmt::Delete(node) => {
            for target in &node.targets {
                validate_delete_target(target)?;
            }
        }
        pyast::Stmt::Assign(node) => {
            for target in &node.targets {
                validate_assign_target(target)?;
            }
        }
        pyast::Stmt::AnnAssign(node) => {
            validate_assign_target(node.target.as_ref())?;
        }
        pyast::Stmt::AugAssign(node) => {
            validate_assign_target(node.target.as_ref())?;
        }
        pyast::Stmt::FunctionDef(node) => {
            validate_control_flow_stmts(&node.body, true, false)?;
            return Ok(());
        }
        pyast::Stmt::AsyncFunctionDef(node) => {
            validate_control_flow_stmts(&node.body, true, false)?;
            return Ok(());
        }
        pyast::Stmt::ClassDef(node) => {
            validate_control_flow_stmts(&node.body, false, false)?;
            return Ok(());
        }
        pyast::Stmt::If(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::For(node) => {
            validate_control_flow_stmts(&node.body, in_function, true)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::AsyncFor(node) => {
            validate_control_flow_stmts(&node.body, in_function, true)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::While(node) => {
            validate_control_flow_stmts(&node.body, in_function, true)?;
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::With(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::AsyncWith(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::Try(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                validate_control_flow_stmts(&handler.body, in_function, in_loop)?;
            }
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            validate_control_flow_stmts(&node.finalbody, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::TryStar(node) => {
            validate_control_flow_stmts(&node.body, in_function, in_loop)?;
            for handler in &node.handlers {
                let pyast::ExceptHandler::ExceptHandler(handler) = handler;
                validate_control_flow_stmts(&handler.body, in_function, in_loop)?;
            }
            validate_control_flow_stmts(&node.orelse, in_function, in_loop)?;
            validate_control_flow_stmts(&node.finalbody, in_function, in_loop)?;
            return Ok(());
        }
        pyast::Stmt::Match(node) => {
            for case in &node.cases {
                validate_control_flow_stmts(&case.body, in_function, in_loop)?;
            }
            return Ok(());
        }
        _ => {}
    }
    Ok(())
}

#[cfg(feature = "stdlib_ast")]
fn validate_control_flow_stmts(
    stmts: &[pyast::Stmt],
    in_function: bool,
    in_loop: bool,
) -> Result<(), String> {
    for stmt in stmts {
        validate_control_flow_stmt(stmt, in_function, in_loop)?;
    }
    Ok(())
}

#[cfg(feature = "stdlib_ast")]
fn validate_function_scope(
    args: &pyast::Arguments,
    body: &[pyast::Stmt],
    enclosing_function_bindings: &[HashSet<String>],
) -> Result<(), String> {
    let mut local_bindings: HashSet<String> = HashSet::new();
    collect_arg_bindings(args, &mut local_bindings);
    let mut nonlocal_decls: HashSet<String> = HashSet::new();
    let mut global_decls: HashSet<String> = HashSet::new();
    for stmt in body {
        collect_function_scope_info(
            stmt,
            &mut local_bindings,
            &mut nonlocal_decls,
            &mut global_decls,
        );
    }
    for name in &nonlocal_decls {
        if global_decls.contains(name) {
            return Err(format!("name '{name}' is nonlocal and global"));
        }
        let mut found = false;
        for scope in enclosing_function_bindings.iter().rev() {
            if scope.contains(name) {
                found = true;
                break;
            }
        }
        if !found {
            return Err(format!("no binding for nonlocal '{name}' found"));
        }
    }
    for name in &nonlocal_decls {
        local_bindings.remove(name);
    }
    for name in &global_decls {
        local_bindings.remove(name);
    }
    let mut next_enclosing = enclosing_function_bindings.to_vec();
    next_enclosing.push(local_bindings);
    walk_nested_function_scopes(body, &next_enclosing)
}

#[cfg(feature = "stdlib_ast")]
fn compile_validate_nonlocal_semantics(parsed: &pyast::Mod) -> Result<(), String> {
    match parsed {
        pyast::Mod::Module(module) => {
            validate_control_flow_stmts(&module.body, false, false)?;
            walk_nested_function_scopes(&module.body, &[])
        }
        pyast::Mod::Interactive(module) => {
            validate_control_flow_stmts(&module.body, false, false)?;
            walk_nested_function_scopes(&module.body, &[])
        }
        _ => Ok(()),
    }
}

#[cfg(feature = "stdlib_ast")]
fn compile_validate_source(
    source: &str,
    filename: &str,
    mode: &str,
) -> Result<(), (&'static str, String)> {
    let parse_mode = match mode {
        "exec" => ParseMode::Module,
        "eval" => ParseMode::Expression,
        "single" => ParseMode::Interactive,
        _ => {
            return Err((
                "ValueError",
                "compile() mode must be 'exec', 'eval' or 'single'".to_string(),
            ));
        }
    };
    match parse_python(source, parse_mode, filename) {
        Ok(parsed) => match compile_validate_nonlocal_semantics(&parsed) {
            Ok(()) => Ok(()),
            Err(message) => Err(("SyntaxError", message)),
        },
        Err(err) => Err((compile_error_type(&err.error), err.error.to_string())),
    }
}

#[unsafe(no_mangle)]
#[cfg(feature = "stdlib_ast")]
pub extern "C" fn molt_compile_builtin(
    source_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    flags_bits: u64,
    dont_inherit_bits: u64,
    optimize_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 1 must be a string");
            }
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 2 must be a string");
            }
        };
        let mode = match string_obj_to_owned(obj_from_bits(mode_bits)) {
            Some(val) => val,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 3 must be a string");
            }
        };
        if mode != "exec" && mode != "eval" && mode != "single" {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "compile() mode must be 'exec', 'eval' or 'single'",
            );
        }
        if to_i64(obj_from_bits(flags_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 4 must be int");
        }
        if to_i64(obj_from_bits(dont_inherit_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 5 must be int");
        }
        if to_i64(obj_from_bits(optimize_bits)).is_none() {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 6 must be int");
        }
        if let Err((error_type, message)) = compile_validate_source(&source, &filename, &mode) {
            return raise_exception::<_>(_py, error_type, &message);
        }
        codeobj_from_filename_bits(_py, filename_bits)
    })
}

#[unsafe(no_mangle)]
#[cfg(feature = "stdlib_ast")]
pub extern "C" fn molt_codeop_compile(
    source_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    flags_bits: u64,
    incomplete_input_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 1 must be a string");
            }
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 2 must be a string");
            }
        };
        let mode = match string_obj_to_owned(obj_from_bits(mode_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 3 must be a string");
            }
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 4 must be int");
        };
        let incomplete_input = is_truthy(_py, obj_from_bits(incomplete_input_bits));
        match codeop_compile_status(&source, &filename, &mode, flags, incomplete_input) {
            CodeopCompileStatus::Compiled { next_flags } => {
                let code_bits = codeobj_from_filename_bits(_py, filename_bits);
                if obj_from_bits(code_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let flags_out_bits = MoltObject::from_int(next_flags).bits();
                let result_ptr = alloc_tuple(_py, &[code_bits, flags_out_bits]);
                dec_ref_bits(_py, code_bits);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(result_ptr).bits()
            }
            CodeopCompileStatus::Incomplete => {
                raise_exception::<_>(_py, "SyntaxError", "incomplete input")
            }
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => raise_exception::<_>(_py, error_type, &message),
        }
    })
}

#[unsafe(no_mangle)]
#[cfg(feature = "stdlib_ast")]
pub extern "C" fn molt_codeop_compile_command(
    source_bits: u64,
    filename_bits: u64,
    mode_bits: u64,
    flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 1 must be a string");
            }
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 2 must be a string");
            }
        };
        let mode = match string_obj_to_owned(obj_from_bits(mode_bits)) {
            Some(value) => value,
            None => {
                return raise_exception::<_>(_py, "TypeError", "compile() arg 3 must be a string");
            }
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "compile() arg 4 must be int");
        };

        let mut only_blank_or_comment = true;
        for line in source.split('\n') {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                only_blank_or_comment = false;
                break;
            }
        }
        if only_blank_or_comment && mode != "eval" {
            source = "pass".to_string();
        }
        if codeop_source_has_missing_indented_suite(&source) {
            return raise_exception::<_>(_py, "SyntaxError", "expected an indented block");
        }

        match codeop_compile_status(&source, &filename, &mode, flags, true) {
            CodeopCompileStatus::Compiled { next_flags } => {
                let code_bits = codeobj_from_filename_bits(_py, filename_bits);
                if obj_from_bits(code_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let flags_out_bits = MoltObject::from_int(next_flags).bits();
                let result_ptr = alloc_tuple(_py, &[code_bits, flags_out_bits]);
                dec_ref_bits(_py, code_bits);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(result_ptr).bits();
            }
            CodeopCompileStatus::Incomplete => {}
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => {
                if error_type != "SyntaxError" {
                    return raise_exception::<_>(_py, error_type, &message);
                }
            }
        }

        let source_newline = format!("{source}\n");
        match codeop_compile_status(&source_newline, &filename, &mode, flags, true) {
            CodeopCompileStatus::Compiled { .. } | CodeopCompileStatus::Incomplete => {
                let flags_out_bits = MoltObject::from_int(flags).bits();
                let result_ptr = alloc_tuple(_py, &[MoltObject::none().bits(), flags_out_bits]);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(result_ptr).bits();
            }
            CodeopCompileStatus::Error {
                error_type: "SyntaxError",
                ..
            } => {}
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => return raise_exception::<_>(_py, error_type, &message),
        }

        match codeop_compile_status(&source, &filename, &mode, flags, false) {
            CodeopCompileStatus::Compiled { next_flags } => {
                let code_bits = codeobj_from_filename_bits(_py, filename_bits);
                if obj_from_bits(code_bits).is_none() {
                    return MoltObject::none().bits();
                }
                let flags_out_bits = MoltObject::from_int(next_flags).bits();
                let result_ptr = alloc_tuple(_py, &[code_bits, flags_out_bits]);
                dec_ref_bits(_py, code_bits);
                if result_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(result_ptr).bits()
            }
            CodeopCompileStatus::Incomplete => {
                raise_exception::<_>(_py, "SyntaxError", "incomplete input")
            }
            CodeopCompileStatus::Error {
                error_type,
                message,
            } => raise_exception::<_>(_py, error_type, &message),
        }
    })
}

// --- Stubs when stdlib_ast is disabled ---

#[cfg(not(feature = "stdlib_ast"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_compile_builtin(
    _source_bits: u64,
    _filename_bits: u64,
    _mode_bits: u64,
    _flags_bits: u64,
    _dont_inherit_bits: u64,
    _optimize_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "NotImplementedError",
            "compile() requires the stdlib_ast feature",
        )
    })
}

#[cfg(not(feature = "stdlib_ast"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_codeop_compile(
    _source_bits: u64,
    _filename_bits: u64,
    _mode_bits: u64,
    _flags_bits: u64,
    _incomplete_input_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "NotImplementedError",
            "compile() requires the stdlib_ast feature",
        )
    })
}

#[cfg(not(feature = "stdlib_ast"))]
#[unsafe(no_mangle)]
pub extern "C" fn molt_codeop_compile_command(
    _source_bits: u64,
    _filename_bits: u64,
    _mode_bits: u64,
    _flags_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        raise_exception::<u64>(
            _py,
            "NotImplementedError",
            "compile() requires the stdlib_ast feature",
        )
    })
}
