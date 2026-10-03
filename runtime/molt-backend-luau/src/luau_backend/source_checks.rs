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

// Block keywords must come from code, never from diagnostics or string values.
// Preserve byte positions while masking quoted literals and removing comments.
pub(super) fn luau_line_code(line: &str) -> String {
    let mut code = String::with_capacity(line.len());
    let mut quote = None;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            code.extend(std::iter::repeat_n(' ', ch.len_utf8()));
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == '\\' {
                escaped = true;
            } else if ch == active_quote {
                quote = None;
            }
            code.extend(std::iter::repeat_n(' ', ch.len_utf8()));
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            code.push(' ');
            continue;
        }
        if ch == '-' && line[idx..].starts_with("--") {
            break;
        }
        code.push(ch);
    }
    code
}

fn opens_luau_function_block(trimmed: &str) -> bool {
    (trimmed.starts_with("local function ")
        || trimmed.starts_with("function ")
        || trimmed.contains("= function(")
        || trimmed.contains("function(")
        || trimmed.starts_with("return function("))
        && !trimmed.contains(" end")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LuauBlockHeader {
    If,
    ElseIf,
    Loop,
}

impl LuauBlockHeader {
    fn delimiter(self) -> &'static str {
        if self == Self::Loop { "do" } else { "then" }
    }
}

fn luau_code_words(code: &str) -> impl Iterator<Item = &str> {
    code.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
}

fn luau_block_header(code: &str) -> Option<LuauBlockHeader> {
    match luau_code_words(code).next()? {
        "if" => Some(LuauBlockHeader::If),
        "elseif" => Some(LuauBlockHeader::ElseIf),
        "for" | "while" => Some(LuauBlockHeader::Loop),
        _ => None,
    }
}

// This checker admits the backend's statement-line structure, including header
// continuations. It does not parse expression grammar: inline function bodies
// and conditional expressions remain Luau syntax owned by the guest parser.
// Only a standalone structural boundary can interrupt a pending header; an
// `end` inside `if function() return true end then` belongs to the expression.
fn luau_header_boundary(code: &str) -> bool {
    is_luau_end_line(code)
        || matches!(
            luau_code_words(code).next(),
            Some("else" | "elseif" | "until")
        )
}

fn luau_header_complete(code: &str, header: LuauBlockHeader) -> bool {
    luau_code_words(code).any(|word| word == header.delimiter())
}

fn is_luau_end_line(trimmed: &str) -> bool {
    trimmed == "end" || trimmed.starts_with("end)") || trimmed.starts_with("end,")
}

fn validate_luau_block_structure(source: &str) -> Result<(), String> {
    let mut stack: Vec<(LuauBlockKind, usize)> = Vec::new();
    let mut pending_header: Option<(LuauBlockHeader, usize, String)> = None;

    for (line_index, raw_line) in source.lines().enumerate() {
        let mut line_number = line_index + 1;
        let code = luau_line_code(raw_line);
        if code.trim().is_empty() {
            continue;
        }
        let (header, code) = if let Some((header, opened_line, mut previous)) =
            pending_header.take()
        {
            if luau_header_boundary(code.trim()) {
                return Err(format!(
                    "luau block structure error at line {opened_line}: header is missing `{}` before `{}` at line {line_number}",
                    header.delimiter(),
                    code.trim()
                ));
            }
            line_number = opened_line;
            previous.push(' ');
            previous.push_str(code.trim());
            (Some(header), previous)
        } else {
            (luau_block_header(code.trim()), code.trim().to_owned())
        };
        if let Some(header) = header
            && !luau_header_complete(&code, header)
        {
            pending_header = Some((header, line_number, code));
            continue;
        }
        let trimmed = code.trim();

        // Both continuation branches retain the existing if frame. Either may
        // contain a body and close that frame on the same line; a body without
        // an inline end leaves it open, just like a branch with a separate body.
        let is_else = trimmed == "else" || trimmed.starts_with("else ");
        let is_elseif = header == Some(LuauBlockHeader::ElseIf);
        if is_else || is_elseif {
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
            if trimmed.ends_with(" end") {
                stack.pop();
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
            let closing_function_expression =
                trimmed.starts_with("end)") || trimmed.starts_with("end,");
            match stack.pop() {
                Some((LuauBlockKind::Function, _)) if closing_function_expression => {}
                Some((LuauBlockKind::Function, opened_line)) if trimmed != "end" => {
                    return Err(format!(
                        "luau block structure error at line {line_number}: function block opened at line {opened_line} was closed by unsupported terminator `{trimmed}`"
                    ));
                }
                Some((_kind, _opened_line)) if !closing_function_expression => {}
                Some((kind, opened_line)) => {
                    return Err(format!(
                        "luau block structure error at line {line_number}: `{trimmed}` closes function expression, but top block is {} opened at line {opened_line}",
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

        if let Some(header) = header {
            if !trimmed.ends_with(" end") {
                let kind = if header == LuauBlockHeader::Loop {
                    LuauBlockKind::Loop
                } else {
                    LuauBlockKind::If
                };
                stack.push((kind, line_number));
            }
            continue;
        }
        if opens_luau_function_block(trimmed) {
            stack.push((LuauBlockKind::Function, line_number));
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

    if let Some((header, opened_line, _)) = pending_header {
        let delimiter = header.delimiter();
        return Err(format!(
            "luau block structure error at line {opened_line}: unterminated header missing `{delimiter}`"
        ));
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
    fn test_validate_luau_source_accepts_generic_callback_and_inline_else_close() {
        let source = [
            "--!strict",
            "local function bind(values)",
            "\tscan(values, function(value)",
            "\t\tif value then",
            "\t\t\tif ready then print(value)",
            "\t\t\telse error('not ready') end",
            "\t\telse error('missing') end",
            "\tend)",
            "end",
            "",
        ]
        .join("\n");
        assert!(validate_luau_source(&source).is_ok());
    }

    #[test]
    fn function_expression_may_close_before_later_call_arguments() {
        let source = [
            "local function invoke()",
            "\treturn call_with_context(function()",
            "\t\treturn 42",
            "\tend, globals, builtins)",
            "end",
            "",
        ]
        .join("\n");
        validate_luau_source(&source)
            .expect("a function expression may be followed by sibling call arguments");
    }

    #[test]
    fn runtime_provider_fragments_keep_their_block_structure() {
        // In particular CALLABLE_FRAME_RUNTIME's namespace guard has `if` and
        // `then` on different physical lines. Validate the provider authority,
        // not a rewritten one-line surrogate of the failing generated code.
        for (name, source) in super::super::runtime_fragments::fragments() {
            validate_luau_source(&source)
                .unwrap_or_else(|error| panic!("runtime provider {name}: {error}"));
        }
    }

    #[test]
    fn statement_headers_own_continuations_through_then_and_do() {
        let source = r#"
local function visit(values, then_value, do_value)
    if values ~= nil
        -- then end are not header delimiters in a comment.
        and then_value
        and values["then"]
    then
        print("then end")
    elseif then_value
        and values["do"]
    then print(2)
    elseif do_value
    then print(3)
    else print(4) end
    for index,
        value in
        values
    do
        while value
            and do_value
        do value = nil end
    end
    if if then_value then do_value else false then print(5) end
    for _, value in if then_value then values else {}
    do print(value) end
    if function() return true end then
        print(6)
    end
    for value in function() return nil end do
        print(value)
    end
end
"#;
        validate_luau_source(source).expect("physical line breaks do not change block ownership");
    }

    #[test]
    fn incomplete_statement_headers_do_not_consume_other_block_terminators() {
        for (header, delimiter) in [
            ("if ready", "then"),
            ("if ready then\nelseif other", "then"),
            ("for index in values", "do"),
            ("while ready", "do"),
        ] {
            let source = format!("local function f()\n{header}\n-- {delimiter}\n");
            let error = validate_luau_source(&source).expect_err("header needs its delimiter");
            assert!(error.contains(&format!("missing `{delimiter}`")), "{error}");
            assert!(error.contains("unterminated header"), "{error}");
            let terminated = format!("{source}end\n");
            let error = validate_luau_source(&terminated)
                .expect_err("an outer end cannot silently close an incomplete header");
            assert!(
                error.contains(&format!("missing `{delimiter}` before `end`")),
                "{error}"
            );
        }
        for source in [
            "if ready\nthen\nend\nend\n",
            "for index in values\ndo\nend\nend\n",
            "while ready\ndo\nend\nend\n",
        ] {
            let error = validate_luau_source(source).expect_err("extra block terminator");
            assert!(error.contains("orphan `end`"), "{error}");
        }
        let error = validate_luau_source("elseif ready\nthen print(1) end\n")
            .expect_err("a continued elseif still needs its owning if");
        assert!(error.contains("orphan"), "{error}");
    }

    #[test]
    fn if_chain_continuations_share_block_custody_for_every_body_layout() {
        let chains = [
            "if first then\nprint(1)\nelseif second then\nprint(2)\nelse\nprint(3)\nend",
            "if first then print(1)\nelseif second then print(2) end",
            "if first then print(1)\nelseif second then print(2)\nend",
            "if first then print(1)\nelseif second then print(2)\nelse print(3) end",
            "if first then print(1)\nelse print(2)\nend",
            "if first then print(1)\nelseif second then print(2)\nelseif third then print(3) end",
        ];
        for chain in chains {
            let source = format!(
                "local function outer()\nscan(values, function(value)\n{chain}\nend)\nend\n"
            );
            validate_luau_source(&source)
                .unwrap_or_else(|error| panic!("valid chain rejected: {error}\n{source}"));
        }
    }

    #[test]
    fn if_chain_keywords_in_literals_and_comments_do_not_change_block_custody() {
        let source = r#"
local function visit()
    if first then print("function( -- end")
    elseif second then print(' end') -- end
    else print("quote: \" end") end -- function(
end
"#;
        validate_luau_source(source).expect("only code may open or close blocks");

        let malformed = "local function visit()\nif first then print(' end')\nend\n";
        let error = validate_luau_source(malformed)
            .expect_err("a quoted end must not hide a missing block terminator");
        assert!(error.contains("unterminated function block"), "{error}");
    }

    #[test]
    fn if_chain_continuations_reject_orphan_and_mismatched_branches() {
        for branch in [
            "else",
            "else print(1)",
            "else print(1) end",
            "elseif flag then",
            "elseif flag then print(1)",
            "elseif flag then print(1) end",
        ] {
            let orphan = validate_luau_source(branch)
                .expect_err("a continuation must have an owning if block");
            assert!(orphan.contains("orphan"), "{orphan}");
            for opener in ["local function f()", "while ready do", "do", "repeat"] {
                let source = format!("{opener}\n{branch}\nend\n");
                let error = validate_luau_source(&source)
                    .expect_err("a continuation cannot steal another block's terminator");
                assert!(error.contains("belongs to if block"), "{error}");
            }
        }
    }

    #[test]
    fn inline_if_chain_closures_still_reject_missing_and_extra_ends() {
        for chain in [
            "if first then print(1)\nelseif second then print(2)",
            "if first then print(1)\nelse print(2)",
        ] {
            let source = format!("local function f()\n{chain}\nend\n");
            let error = validate_luau_source(&source).expect_err("missing if terminator");
            assert!(error.contains("unterminated function block"), "{error}");
        }
        for branch in ["elseif second then print(2) end", "else print(2) end"] {
            let source = format!("if first then print(1)\n{branch}\nend\n");
            let error = validate_luau_source(&source).expect_err("extra if terminator");
            assert!(error.contains("orphan `end`"), "{error}");
        }
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
