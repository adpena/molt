use std::fmt::Write;

fn collect_luau_preview_blockers(source: &str) -> Vec<String> {
    source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.contains("-- [unsupported op:")
                || trimmed.contains("error(\"[unsupported op:")
            {
                return Some(format!("unsupported marker: {trimmed}"));
            }

            let semantic_stub = trimmed.contains("-- [async:")
                || trimmed.contains("-- [file:")
                || trimmed.contains("-- [context:")
                || trimmed.contains("-- [internal:")
                || trimmed.contains("-- [stub:")
                || trimmed.contains("-- [class op:")
                || trimmed.contains("-- [try_start]")
                || trimmed.contains("-- [try_end]")
                || (trimmed.contains(" = nil -- [")
                    && !trimmed.contains("-- [exception_last]")
                    && !trimmed.contains("-- [exception_message]")
                    && !trimmed.contains("-- [missing]"));
            if semantic_stub {
                Some(format!("semantic stub marker: {trimmed}"))
            } else {
                None
            }
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LuauBlockKind {
    Function,
    If,
    Loop,
    Do,
    Repeat,
}

fn luau_block_kind_name(kind: LuauBlockKind) -> &'static str {
    match kind {
        LuauBlockKind::Function => "function",
        LuauBlockKind::If => "if",
        LuauBlockKind::Loop => "loop",
        LuauBlockKind::Do => "do",
        LuauBlockKind::Repeat => "repeat",
    }
}

fn strip_luau_line_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }
        if ch == '-' && line[idx..].starts_with("--") {
            return &line[..idx];
        }
    }
    line
}

fn opens_luau_function_block(trimmed: &str) -> bool {
    (trimmed.starts_with("local function ")
        || trimmed.starts_with("function ")
        || trimmed.contains("= function(")
        || trimmed.contains("pcall(function(")
        || trimmed.contains("xpcall(function(")
        || trimmed.starts_with("return function("))
        && !trimmed.contains(" end")
}

fn opens_luau_if_block(trimmed: &str) -> bool {
    trimmed.starts_with("if ") && trimmed.ends_with(" then")
}

fn opens_luau_loop_block(trimmed: &str) -> bool {
    (trimmed.starts_with("for ") || trimmed.starts_with("while ")) && trimmed.ends_with(" do")
}

fn is_luau_end_line(trimmed: &str) -> bool {
    trimmed == "end" || trimmed.starts_with("end)") || trimmed.starts_with("end,")
}

fn validate_luau_block_structure(source: &str) -> Result<(), String> {
    let mut stack: Vec<(LuauBlockKind, usize)> = Vec::new();

    for (line_index, raw_line) in source.lines().enumerate() {
        let line_number = line_index + 1;
        let trimmed = strip_luau_line_comment(raw_line).trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed == "else" || (trimmed.starts_with("elseif ") && trimmed.ends_with(" then")) {
            match stack.last() {
                Some((LuauBlockKind::If, _)) => {}
                Some((kind, opened_line)) => {
                    return Err(format!(
                        "luau block structure error at line {line_number}: `{trimmed}` belongs to if block, but top block is {} opened at line {opened_line}",
                        luau_block_kind_name(*kind)
                    ));
                }
                None => {
                    return Err(format!(
                        "luau block structure error at line {line_number}: orphan `{trimmed}`"
                    ));
                }
            }
            continue;
        }

        if trimmed.starts_with("until ") {
            match stack.pop() {
                Some((LuauBlockKind::Repeat, _)) => {}
                Some((kind, opened_line)) => {
                    return Err(format!(
                        "luau block structure error at line {line_number}: `until` closes repeat block, but top block is {} opened at line {opened_line}",
                        luau_block_kind_name(kind)
                    ));
                }
                None => {
                    return Err(format!(
                        "luau block structure error at line {line_number}: orphan `until`"
                    ));
                }
            }
            continue;
        }

        if is_luau_end_line(trimmed) {
            let closing_pcall_function = trimmed.starts_with("end)");
            match stack.pop() {
                Some((LuauBlockKind::Function, _)) if closing_pcall_function => {}
                Some((LuauBlockKind::Function, opened_line)) if trimmed != "end" => {
                    return Err(format!(
                        "luau block structure error at line {line_number}: function block opened at line {opened_line} was closed by unsupported terminator `{trimmed}`"
                    ));
                }
                Some((_kind, _opened_line)) if !closing_pcall_function => {}
                Some((kind, opened_line)) => {
                    return Err(format!(
                        "luau block structure error at line {line_number}: `end)` closes pcall function, but top block is {} opened at line {opened_line}",
                        luau_block_kind_name(kind)
                    ));
                }
                None => {
                    return Err(format!(
                        "luau block structure error at line {line_number}: orphan `end`"
                    ));
                }
            }
            continue;
        }

        if opens_luau_function_block(trimmed) {
            stack.push((LuauBlockKind::Function, line_number));
            continue;
        }
        if opens_luau_if_block(trimmed) {
            stack.push((LuauBlockKind::If, line_number));
            continue;
        }
        if opens_luau_loop_block(trimmed) {
            stack.push((LuauBlockKind::Loop, line_number));
            continue;
        }
        if trimmed == "do" {
            stack.push((LuauBlockKind::Do, line_number));
            continue;
        }
        if trimmed == "repeat" {
            stack.push((LuauBlockKind::Repeat, line_number));
        }
    }

    if let Some((kind, opened_line)) = stack.last() {
        return Err(format!(
            "luau block structure error: unterminated {} block opened at line {opened_line}",
            luau_block_kind_name(*kind)
        ));
    }

    Ok(())
}

pub fn validate_luau_source(source: &str) -> Result<(), String> {
    let blockers = collect_luau_preview_blockers(source);
    if blockers.is_empty() {
        return validate_luau_block_structure(source);
    }
    let mut message = format!(
        "luau preview backend rejected lowered output with {} unsupported marker{}:",
        blockers.len(),
        if blockers.len() == 1 { "" } else { "s" }
    );
    for blocker in blockers.iter().take(8) {
        let _ = write!(message, "\n- {blocker}");
    }
    if blockers.len() > 8 {
        let _ = write!(message, "\n- ... {} more", blockers.len() - 8);
    }
    Err(message)
}

/// Performance review of emitted Luau source.
///
/// Returns a report of remaining perf opportunities that an agent or human
/// reviewer can act on before the next pipeline phase (deploy, Studio MCP, etc.).
/// Each entry is a (line_number, category, message) triple.
pub fn review_luau_perf(source: &str) -> Vec<(usize, &'static str, String)> {
    let mut issues = Vec::new();
    let has_file_native = source.lines().any(|l| l.trim() == "--!native");
    for (i, line) in source.lines().enumerate() {
        let trimmed = line.trim();
        let ln = i + 1;

        // Remaining helper calls that should have been inlined.
        if trimmed.contains("molt_pow(") {
            issues.push((
                ln,
                "helper-call",
                "molt_pow() not inlined — use a ^ b".into(),
            ));
        }
        if trimmed.contains("molt_floor_div(") {
            issues.push((
                ln,
                "helper-call",
                "molt_floor_div() not inlined — use a // b (LOP_IDIV)".into(),
            ));
        }
        if trimmed.contains("molt_mod(") {
            issues.push((
                ln,
                "helper-call",
                "molt_mod() not inlined — use a % b".into(),
            ));
        }

        // Type-checked add that could be numeric.
        if trimmed.contains("if type(") && trimmed.contains("then tostring(") {
            issues.push((
                ln,
                "type-check",
                "type-checked add — verify if operands are numeric".into(),
            ));
        }

        // table.insert in user code (not in helper definitions).
        if trimmed.contains("table.insert(") && !trimmed.starts_with("--") {
            issues.push((
                ln,
                "table-insert",
                "table.insert() — use result[n] = x for speed".into(),
            ));
        }

        // Missing @native on `local function` definitions (only syntax that supports @native).
        // Skip this check if the file has --!native directive (enables native for all functions).
        if !has_file_native && trimmed.starts_with("local function ") && !trimmed.starts_with("--")
        {
            // Check if previous line has @native.
            if i == 0
                || source
                    .lines()
                    .nth(i - 1)
                    .is_none_or(|prev| prev.trim() != "@native")
            {
                // Don't flag runtime helper definitions.
                if !trimmed.contains("molt_range")
                    && !trimmed.contains("molt_len")
                    && !trimmed.contains("molt_int")
                    && !trimmed.contains("molt_float")
                    && !trimmed.contains("molt_str")
                    && !trimmed.contains("molt_bool")
                {
                    issues.push((ln, "native", "function missing @native annotation".into()));
                }
            }
        }

        // Unsupported ops that are still present.
        if trimmed.contains("-- [unsupported op:") {
            issues.push((ln, "unsupported", trimmed.to_string()));
        }

        // Note: Python `is` on non-None operands maps to == (value equality,
        // not identity).  This is an accepted semantic gap — no inline marker
        // is emitted because comments break when inlined by optimization passes.
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::{review_luau_perf, validate_luau_source};

    #[test]
    fn test_validate_luau_source_accepts_plain_output() {
        let source = "--!strict\nfunction molt_main()\n\tprint(42)\nend\n";
        assert!(validate_luau_source(source).is_ok());
    }

    #[test]
    fn test_validate_luau_source_accepts_pcall_function_block() {
        let source = [
            "--!strict",
            "function molt_main()",
            "\tlocal __ok_0, __err_0 = pcall(function()",
            "\t\tif flag then",
            "\t\t\tprint(1)",
            "\t\telse",
            "\t\t\tprint(2)",
            "\t\tend",
            "\tend)",
            "\tif not __ok_0 then",
            "\t\terror(__err_0)",
            "\tend",
            "end",
            "",
        ]
        .join("\n");
        assert!(validate_luau_source(&source).is_ok());
    }

    #[test]
    fn test_validate_luau_source_rejects_orphan_end() {
        let err = validate_luau_source("--!strict\nfunction molt_main()\nend\nend\n")
            .expect_err("extra end should be rejected");
        assert!(err.contains("orphan `end`"));
    }

    #[test]
    fn test_validate_luau_source_rejects_unterminated_block() {
        let err = validate_luau_source("--!strict\nfunction molt_main()\n\tif flag then\nend\n")
            .expect_err("unterminated function should be rejected");
        assert!(err.contains("unterminated function block"));
    }

    #[test]
    fn test_validate_luau_source_rejects_semantic_stub_comments() {
        let markers = [
            "local v0 = nil -- [async: spawn]",
            "local v0 = nil -- [file: file_open]",
            "local v0 = nil -- [context: context_enter]",
            "local v0 = nil -- [internal: function_closure_bits]",
            "local v0 = true -- [stub: isinstance]",
            "-- [class op: class_merge_layout]",
            "local v0 = nil -- [bridge_unavailable]",
        ];
        for marker in markers {
            let source = format!("--!strict\nfunction molt_main()\n\t{marker}\nend\n");
            let err =
                validate_luau_source(&source).expect_err("semantic stub marker should be rejected");
            assert!(err.contains("semantic stub marker"));
            assert!(err.contains(marker));
        }
    }

    #[test]
    fn test_validate_luau_source_rejects_unsupported_op() {
        let err = validate_luau_source(
            "--!strict\nfunction molt_main()\n\tlocal v0 = nil -- [unsupported op: foo]\nend\n",
        )
        .expect_err("unsupported op marker should be rejected");
        assert!(err.contains("unsupported marker"));
        assert!(err.contains("[unsupported op: foo]"));
    }

    #[test]
    fn test_review_luau_perf_reports_source_level_authority_categories() {
        let issues = review_luau_perf(
            "--!strict\nfunction molt_main()\n\tlocal x = molt_pow(a, b)\n\ttable.insert(xs, x)\n\t-- [unsupported op: foo]\nend\n",
        );
        let categories: Vec<_> = issues.iter().map(|(_, category, _)| *category).collect();
        assert!(categories.contains(&"helper-call"));
        assert!(categories.contains(&"table-insert"));
        assert!(categories.contains(&"unsupported"));
    }

    #[test]
    fn test_review_luau_perf_honors_file_native_directive() {
        let issues = review_luau_perf("--!native\nlocal function user_func()\n\treturn 1\nend\n");
        assert!(!issues.iter().any(|(_, category, _)| *category == "native"));
    }
}
