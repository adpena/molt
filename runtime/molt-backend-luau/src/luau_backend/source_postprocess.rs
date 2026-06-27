use std::collections::{BTreeMap, BTreeSet};

#[path = "source_postprocess/cleanup_artifacts.rs"]
mod cleanup_artifacts;
#[path = "source_postprocess/control_flow.rs"]
mod control_flow;
#[path = "source_postprocess/local_values.rs"]
mod local_values;
#[path = "source_postprocess/loop_folds.rs"]
mod loop_folds;
#[path = "source_postprocess/source_text.rs"]
mod source_text;

use cleanup_artifacts::{
    eliminate_nil_missing_wrappers, strip_dead_locals_dict_stores, strip_unbound_local_checks,
};
use control_flow::{
    eliminate_goto_labels, rehoist_escaped_locals, strip_dead_code_after_terminators,
    strip_dead_gotos_and_labels, strip_exception_cleanup_blocks, structure_forward_if_else_blocks,
    structure_pcall_failure_blocks,
};
use local_values::{
    inline_single_use_constants, optimize_multi_return, propagate_single_use_copies,
    simplify_return_chain, sink_single_use_locals, strip_undefined_rhs_assignments,
};
use loop_folds::{fold_range_indices, simplify_comparison_break, strip_trailing_continue};
use source_text::{find_matching_paren, is_ident_char, is_pure_expr};

pub(super) fn optimize_luau_source(source: &mut String) {
    // Phase 2a: Strip dead exception boilerplate early, but only the
    // simple nil-check patterns. More aggressive cleanup happens later.
    strip_exception_cleanup_blocks(source);
    strip_dead_gotos_and_labels(source);

    // Phase 2b: Core optimization passes.
    inline_single_use_constants(source);
    eliminate_nil_missing_wrappers(source);
    strip_unbound_local_checks(source);
    strip_dead_locals_dict_stores(source);
    strip_undefined_rhs_assignments(source);
    propagate_single_use_copies(source);
    strip_trailing_continue(source);
    simplify_comparison_break(source);
    optimize_luau_perf(source);
    // Second copy-prop pass: optimize_luau_perf reduces type-guard
    // expressions, unlocking more copy propagation.
    propagate_single_use_copies(source);
    eliminate_common_subexpressions(source);
    hoist_loop_invariants(source);
    sink_single_use_locals(source);
    simplify_return_chain(source);
    optimize_multi_return(source);
    fold_range_indices(source);

    // Phase 2c: Final cleanup. Re-strip exception blocks that survived
    // initial cleanup, then re-run key passes that benefit from cleaner code.
    strip_exception_cleanup_blocks(source);
    strip_dead_gotos_and_labels(source);
    inline_single_use_constants(source);
    propagate_single_use_copies(source);
    sink_single_use_locals(source);
    rehoist_escaped_locals(source);
    structure_pcall_failure_blocks(source);
    structure_forward_if_else_blocks(source);

    // Phase 2d: Convert goto/label pairs to structured control flow.
    eliminate_goto_labels(source);

    // Phase 2e: Strip dead code after terminators.
    strip_dead_code_after_terminators(source);

    // Phase 2f: Insert do/end blocks for functions exceeding the local limit.
    spill_excess_locals(source);
}

fn eliminate_common_subexpressions(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: collect `local vN = <pure_expr>` keyed by (expr, indent).
    let mut expr_map: BTreeMap<String, Vec<(usize, String)>> = BTreeMap::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("local v")
            && let Some(eq_pos) = rest.find(" = ")
        {
            let before_eq = &rest[..eq_pos];
            // Strip optional type annotation (": number", etc.)
            let suffix = if let Some(colon) = before_eq.find(':') {
                &before_eq[..colon]
            } else {
                before_eq
            };
            if suffix.chars().all(|c| c.is_ascii_digit()) {
                let var = format!("v{suffix}");
                let rhs = rest[eq_pos + 3..].trim();
                if is_pure_expr(rhs) && rhs.len() > 3 {
                    let indent = line.len() - trimmed.len();
                    let key = format!("{}@{}", rhs, indent);
                    expr_map.entry(key).or_default().push((i, var));
                }
            }
        }
    }

    // Phase 2: for expressions with 2+ occurrences, replace later ones.
    let mut replacements: BTreeMap<usize, String> = BTreeMap::new();
    let mut count: usize = 0;

    for occurrences in expr_map.values() {
        if occurrences.len() < 2 {
            continue;
        }
        let (first_line, first_var) = &occurrences[0];

        for (later_line, later_var) in &occurrences[1..] {
            // Verify first_var is not reassigned between the two sites.
            let mut reassigned = false;
            // Also check for block boundaries: if an `end` at the same or
            // shallower indent appears between the sites, the first variable
            // went out of scope (different block at same depth).
            let first_indent = lines[*first_line].len() - lines[*first_line].trim().len();
            let mut scope_broken = false;
            for j in (*first_line + 1)..*later_line {
                let t = lines[j].trim();
                if t.starts_with(&format!("{first_var} = ")) {
                    reassigned = true;
                    break;
                }
                // Check for block boundary: `end` at same or shallower indent
                if t == "end" {
                    let end_indent = lines[j].len() - t.len();
                    if end_indent <= first_indent {
                        scope_broken = true;
                        break;
                    }
                }
            }
            if reassigned || scope_broken {
                continue;
            }

            // Also verify that none of the vN variables referenced in the
            // expression are reassigned between the two sites.
            let expr_part = {
                let t = lines[*first_line].trim();
                let rest = t.strip_prefix("local v").unwrap();
                let eq = rest.find(" = ").unwrap();
                rest[eq + 3..].trim()
            };
            let mut expr_vars_reassigned = false;
            {
                let bytes = expr_part.as_bytes();
                let mut pos = 0;
                while pos < bytes.len() {
                    if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                        let start = pos;
                        pos += 1;
                        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                            pos += 1;
                        }
                        if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                            let ref_var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                            // Check reassignment between the two sites.
                            for j in (*first_line + 1)..*later_line {
                                let t = lines[j].trim();
                                if t.starts_with(&format!("{ref_var} = ")) {
                                    expr_vars_reassigned = true;
                                    break;
                                }
                            }
                            if expr_vars_reassigned {
                                break;
                            }
                        }
                    } else {
                        pos += 1;
                    }
                }
            }
            if expr_vars_reassigned {
                continue;
            }

            let indent_str =
                &lines[*later_line][..lines[*later_line].len() - lines[*later_line].trim().len()];
            replacements.insert(
                *later_line,
                format!("{indent_str}local {later_var} = {first_var}"),
            );
            count += 1;
        }
    }

    if replacements.is_empty() {
        return;
    }

    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if let Some(replacement) = replacements.get(&i) {
            result.push_str(replacement);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    *source = result;
    eprintln!("[molt-luau] Eliminated {} common subexpressions", count);
}

/// Loop-invariant code motion (LICM).
///
/// Finds `while true do … end` and `for … do … end` loops.  Inside the
/// immediate loop body (one indent level deeper than the loop header),
/// identifies `local vN = <pure_expr>` where *all* referenced variables
/// are defined outside the loop and are never modified inside the loop.
/// Those declarations are hoisted to just before the loop.
fn hoist_loop_invariants(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();
    let mut hoisted_lines: BTreeSet<usize> = BTreeSet::new();
    let mut insertions: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut count: usize = 0;

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        let is_loop =
            trimmed == "while true do" || (trimmed.starts_with("for ") && trimmed.ends_with(" do"));
        if !is_loop {
            i += 1;
            continue;
        }

        let loop_start = i;
        let loop_indent = lines[i].len() - trimmed.len();

        // Find matching `end` at the same indent.
        let mut depth: usize = 1;
        let mut loop_end = i + 1;
        while loop_end < lines.len() && depth > 0 {
            let t = lines[loop_end].trim();
            // Count nesting openers.
            if t == "while true do"
                || (t.starts_with("for ") && t.ends_with(" do"))
                || (t.starts_with("if ") && t.ends_with(" then"))
                || t == "else"
                || (t.starts_with("elseif ") && t.ends_with(" then"))
                || t.starts_with("local function ")
            {
                // `else` / `elseif` don't add depth, they just continue the
                // block opened by `if`.  Only count real block openers.
                if !t.starts_with("else") {
                    depth += 1;
                }
            } else if t == "end" {
                depth -= 1;
            }
            if depth > 0 {
                loop_end += 1;
            }
        }

        if depth != 0 {
            i += 1;
            continue;
        }

        // Collect every variable modified inside the loop body.
        let mut modified_in_loop: BTreeSet<String> = BTreeSet::new();
        for j in (loop_start + 1)..loop_end {
            let t = lines[j].trim();
            // Bare assignment: `vN = ...`
            if t.starts_with('v')
                && let Some(eq) = t.find(" = ")
            {
                let lhs = &t[..eq];
                if lhs.starts_with('v') && lhs[1..].chars().all(|c| c.is_ascii_digit()) {
                    modified_in_loop.insert(lhs.to_string());
                }
            }
            // `local vN = ...` or `local vN: type = ...` also defines vN.
            if let Some(rest) = t.strip_prefix("local v")
                && let Some(eq) = rest.find(" = ")
            {
                let before_eq = &rest[..eq];
                let suffix = if let Some(colon) = before_eq.find(':') {
                    &before_eq[..colon]
                } else {
                    before_eq
                };
                if suffix.chars().all(|c| c.is_ascii_digit()) {
                    modified_in_loop.insert(format!("v{suffix}"));
                }
            }
            // `for` iteration variables.
            if t.starts_with("for ") {
                if let Some(in_pos) = t.find(" in ") {
                    let vars_part = &t[4..in_pos];
                    for v in vars_part.split(", ") {
                        modified_in_loop.insert(v.trim().to_string());
                    }
                }
                // Numeric for: `for vN = ...`
                if let Some(eq_pos) = t.find(" = ") {
                    let var_part = &t[4..eq_pos];
                    if !var_part.contains(' ') {
                        modified_in_loop.insert(var_part.trim().to_string());
                    }
                }
            }
        }

        // Find hoistable declarations at exactly one indent deeper.
        let body_indent = loop_indent + 1;
        for j in (loop_start + 1)..loop_end {
            let t = lines[j].trim();
            let line_indent = lines[j].len() - t.len();

            if line_indent != body_indent {
                continue;
            }

            if let Some(rest) = t.strip_prefix("local v")
                && let Some(eq) = rest.find(" = ")
            {
                let before_eq = &rest[..eq];
                let suffix = if let Some(colon) = before_eq.find(':') {
                    &before_eq[..colon]
                } else {
                    before_eq
                };
                if !suffix.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let var = format!("v{suffix}");
                let rhs = rest[eq + 3..].trim();

                if !is_pure_expr(rhs) {
                    continue;
                }

                // The declared variable itself must not be modified in loop.
                if modified_in_loop.contains(&var) {
                    continue;
                }

                // All vN references in the RHS must not be modified in loop.
                let mut all_invariant = true;
                let bytes = rhs.as_bytes();
                let mut pos = 0;
                while pos < bytes.len() {
                    if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                        let start = pos;
                        pos += 1;
                        while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                            pos += 1;
                        }
                        if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                            let ref_var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                            if modified_in_loop.contains(ref_var) {
                                all_invariant = false;
                                break;
                            }
                        }
                    } else {
                        pos += 1;
                    }
                }

                if !all_invariant {
                    continue;
                }

                // Hoist: emit at the same indent as the loop header.
                let hoist_indent = &lines[loop_start][..loop_indent];
                insertions
                    .entry(loop_start)
                    .or_default()
                    .push(format!("{hoist_indent}{t}"));
                hoisted_lines.insert(j);
                count += 1;
            }
        }

        i += 1;
    }

    if hoisted_lines.is_empty() {
        return;
    }

    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if let Some(hoisted) = insertions.get(&i) {
            for h in hoisted {
                result.push_str(h);
                result.push('\n');
            }
        }
        if !hoisted_lines.contains(&i) {
            result.push_str(line);
            result.push('\n');
        }
    }
    *source = result;
    eprintln!("[molt-luau] Hoisted {} loop-invariant locals", count);
}

/// Performance optimization pass over emitted Luau source.
///
/// Applied after constant inlining and nil-wrapper elimination. Performs:
/// 1. Strength reduction: `x ^ 2` → `x * x`, inline trivial helper calls
/// 2. `@native` annotation on transpiled functions for Luau VM JIT
/// 3. Eliminate redundant type-checked add when operands are provably numeric
/// 4. Inline remaining `molt_pow`/`molt_floor_div` helper calls (from binop path)
///    Simplify `local vN = <int>` + `if type(vN) == "number" then vN + 1 else vN`
///    into a direct integer index. Eliminates the runtime type check when the
///    index is a known integer literal.
fn simplify_numeric_type_guards(source: &mut String) {
    use std::collections::BTreeMap;

    let lines: Vec<&str> = source.lines().collect();
    let mut result = String::with_capacity(source.len());

    // Phase 1: Find `local vN = <integer_literal>` declarations and check
    // if vN is ONLY used in type-guard patterns on the NEXT line.
    let mut int_consts: BTreeMap<String, (usize, i64)> = BTreeMap::new(); // var -> (line, value)
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("local v")
            && let Some(eq_pos) = rest.find(" = ")
        {
            let suffix = &rest[..eq_pos];
            if suffix.chars().all(|c| c.is_ascii_digit()) {
                let rhs = rest[eq_pos + 3..].trim();
                // Check if RHS is a simple integer (possibly negative).
                if let Ok(val) = rhs.parse::<i64>() {
                    let var_name = format!("v{suffix}");
                    int_consts.insert(var_name, (i, val));
                }
            }
        }
    }

    // Phase 2: For each int const, check if the next line contains the
    // type-guard pattern and the const is only used there.
    let mut remove_lines: BTreeSet<usize> = BTreeSet::new();
    let mut line_replacements: BTreeMap<usize, String> = BTreeMap::new();

    for (var, (decl_line, val)) in &int_consts {
        let next_line = decl_line + 1;
        if next_line >= lines.len() {
            continue;
        }

        let pattern = format!("if type({var}) == \"number\" then {var} + 1 else {var}",);
        if lines[next_line].contains(&pattern) {
            // Check the var isn't used elsewhere (only on these 2 lines).
            let mut total_uses = 0;
            for line in &lines {
                let bytes = line.as_bytes();
                let var_bytes = var.as_bytes();
                let mut pos = 0;
                while pos + var_bytes.len() <= bytes.len() {
                    if &bytes[pos..pos + var_bytes.len()] == var_bytes {
                        let before_ok = pos == 0 || !is_ident_char(bytes[pos - 1]);
                        let after_ok = pos + var_bytes.len() >= bytes.len()
                            || !is_ident_char(bytes[pos + var_bytes.len()]);
                        if before_ok && after_ok {
                            total_uses += 1;
                        }
                    }
                    pos += 1;
                }
            }
            // decl (1) + 3 uses in type guard = 4 total
            if total_uses == 4 {
                // Replace the type-guard with the computed index.
                let replacement = format!("{}", val + 1);
                let old_pattern =
                    format!("[if type({var}) == \"number\" then {var} + 1 else {var}]",);
                let new_pattern = format!("[{replacement}]");
                let new_line = lines[next_line].replace(&old_pattern, &new_pattern);
                line_replacements.insert(next_line, new_line);
                remove_lines.insert(*decl_line);
            }
        }
    }

    if remove_lines.is_empty() {
        return; // Nothing to simplify.
    }

    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue;
        }
        if let Some(replacement) = line_replacements.get(&i) {
            result.push_str(replacement);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }

    *source = result;
}

fn optimize_luau_perf(source: &mut String) {
    // Pre-pass: Simplify type-guard index patterns.
    // Pattern: `local vN = <int_literal>` followed by a line containing
    //   `if type(vN) == "number" then vN + 1 else vN`
    // This checks at runtime whether a known-integer index needs +1 adjustment.
    // Since we KNOW vN is numeric, simplify to just `vN + 1` (= literal + 1).
    simplify_numeric_type_guards(source);

    let mut result = String::with_capacity(source.len());
    let mut perf_count: usize = 0;

    // Track which variables are known-numeric (assigned from numeric ops).
    let mut numeric_vars: BTreeSet<String> = BTreeSet::new();

    for line in source.lines() {
        let trimmed = line.trim();
        let mut optimized = line.to_string();

        // Reset numeric tracking at function boundaries to prevent variable
        // name collisions across different function scopes.
        if trimmed.starts_with("function ")
            || trimmed.starts_with("local function ")
            || trimmed.contains("= function(")
        {
            numeric_vars.clear();
        }

        // Skip function definition lines — the inlining passes below must not
        // rewrite `local function molt_xyz(...)` declarations.
        let is_func_def = trimmed.starts_with("local function molt_");

        // Pass 1: Inline molt_pow(a, b) → a ^ b
        if !is_func_def {
            loop {
                let Some(start) = optimized.find("molt_pow(") else {
                    break;
                };
                if let Some(close) = find_matching_paren(&optimized, start + 8) {
                    let inner = &optimized[start + 9..close];
                    if let Some(comma) = inner.find(", ") {
                        let a = inner[..comma].trim();
                        let b = inner[comma + 2..].trim();
                        let replacement = format!("{a} ^ {b}");
                        optimized = format!(
                            "{}{}{}",
                            &optimized[..start],
                            replacement,
                            &optimized[close + 1..]
                        );
                        perf_count += 1;
                        continue;
                    }
                }
                break;
            }
        }

        // Pass 2: Inline molt_floor_div(a, b) → a // b (LOP_IDIV opcode)
        if !is_func_def {
            loop {
                let Some(start) = optimized.find("molt_floor_div(") else {
                    break;
                };
                if let Some(close) = find_matching_paren(&optimized, start + 14) {
                    let inner = &optimized[start + 15..close];
                    if let Some(comma) = inner.find(", ") {
                        let a = inner[..comma].trim();
                        let b = inner[comma + 2..].trim();
                        let replacement = format!("{a} // {b}");
                        optimized = format!(
                            "{}{}{}",
                            &optimized[..start],
                            replacement,
                            &optimized[close + 1..]
                        );
                        perf_count += 1;
                        continue;
                    }
                }
                break;
            }
        }

        // Pass 3: Inline molt_mod(a, b) → a % b
        // Python's floor-mod matches Luau's % for positive divisors, which covers
        // the vast majority of real-world uses (array indexing, hash functions, etc.).
        if !is_func_def {
            loop {
                let Some(start) = optimized.find("molt_mod(") else {
                    break;
                };
                if let Some(close) = find_matching_paren(&optimized, start + 8) {
                    let inner = &optimized[start + 9..close];
                    if let Some(comma) = inner.find(", ") {
                        let a = inner[..comma].trim();
                        let b = inner[comma + 2..].trim();
                        let replacement = format!("{a} % {b}");
                        optimized = format!(
                            "{}{}{}",
                            &optimized[..start],
                            replacement,
                            &optimized[close + 1..]
                        );
                        perf_count += 1;
                        continue;
                    }
                }
                break;
            }
        }

        // Pass 4: Track numeric variables and optimize type-checked add.
        // Handles both `local vN = expr` (with optional type annotation) and
        // bare `vN = expr` assignments.
        // When both operands of a type-checked add are known-numeric, simplify.
        {
            let (is_local, var_name_opt, rhs_opt) =
                if let Some(rest) = trimmed.strip_prefix("local ") {
                    if let Some(eq_pos) = rest.find(" = ") {
                        // Strip optional type annotation from var name
                        let raw_var = &rest[..eq_pos];
                        let var_name = if let Some(colon) = raw_var.find(':') {
                            raw_var[..colon].to_string()
                        } else {
                            raw_var.to_string()
                        };
                        (true, Some(var_name), Some(&rest[eq_pos + 3..]))
                    } else {
                        (true, None, None)
                    }
                } else if trimmed.starts_with('v') {
                    if let Some(eq_pos) = trimmed.find(" = ") {
                        let lhs = &trimmed[..eq_pos];
                        if lhs.starts_with('v') && lhs[1..].chars().all(|c| c.is_ascii_digit()) {
                            (false, Some(lhs.to_string()), Some(&trimmed[eq_pos + 3..]))
                        } else {
                            (false, None, None)
                        }
                    } else {
                        (false, None, None)
                    }
                } else {
                    (false, None, None)
                };

            if let (Some(var_name), Some(rhs)) = (var_name_opt, rhs_opt) {
                // Detect numeric assignment patterns.
                let is_numeric_rhs = rhs.parse::<f64>().is_ok()
                    || rhs.starts_with("math")
                    || rhs.starts_with("bit32")
                    || rhs.contains(" + ")
                    || rhs.contains(" - ")
                    || rhs.contains(" * ")
                    || rhs.contains(" / ")
                    || rhs.contains(" ^ ")
                    || rhs.contains(" % ")
                    || rhs.starts_with("molt_int(")
                    || rhs.starts_with("molt_float(")
                    || rhs.starts_with("molt_len(")
                    || rhs.starts_with("#")
                    || rhs.starts_with("tonumber(")
                    // A variable copy from a known-numeric var is also numeric.
                    || (rhs.starts_with('v') && rhs[1..].chars().all(|c| c.is_ascii_digit())
                        && numeric_vars.contains(rhs));
                if is_numeric_rhs {
                    numeric_vars.insert(var_name.clone());
                }

                // Check for type-checked add that can be simplified.
                if rhs.starts_with("if type(")
                    && rhs.contains("then tostring(")
                    && rhs.contains("else ")
                    && let Some(else_pos) = rhs.rfind("else ")
                {
                    let numeric_expr = &rhs[else_pos + 5..];
                    if let Some(plus) = numeric_expr.find(" + ") {
                        let lhs_var = numeric_expr[..plus].trim();
                        let rhs_var = numeric_expr[plus + 3..].trim();
                        if numeric_vars.contains(lhs_var) && numeric_vars.contains(rhs_var) {
                            let indent = &line[..line.len() - trimmed.len()];
                            if is_local {
                                optimized = format!("{indent}local {var_name} = {numeric_expr}");
                            } else {
                                optimized = format!("{indent}{var_name} = {numeric_expr}");
                            }
                            numeric_vars.insert(var_name);
                            perf_count += 1;
                        }
                    }
                }
            }
        }

        // Pass 4b: Simplify index type-guards for known-numeric variables.
        // Pattern: `[if type(vN) == "number" then vN + 1 else vN]` → `[vN + 1]`
        while optimized.contains("if type(") && optimized.contains("+ 1 else") {
            let search = "if type(";
            let Some(start) = optimized.find(search) else {
                break;
            };
            // Check bracket context: must be inside `[...]`
            let bracket_start = if start > 0 && optimized.as_bytes()[start - 1] == b'[' {
                start - 1
            } else {
                break;
            };
            // Extract var name from `if type(vN) ==`
            let after_type = &optimized[start + search.len()..];
            let Some(close_paren) = after_type.find(')') else {
                break;
            };
            let var = &after_type[..close_paren];
            if !var.starts_with('v') || !var[1..].chars().all(|c| c.is_ascii_digit()) {
                break;
            }
            // Verify full pattern
            let full_pattern = format!("[if type({var}) == \"number\" then {var} + 1 else {var}]");
            if !optimized[bracket_start..].starts_with(&full_pattern) {
                break;
            }
            if numeric_vars.contains(var) {
                let replacement = format!("[{var} + 1]");
                optimized = optimized.replacen(&full_pattern, &replacement, 1);
                perf_count += 1;
                continue; // Check for more on same line
            }
            break;
        }

        // Pass 5: Strength reduce x ^ 2 → x * x (only for literal 2).
        if optimized.contains(" ^ 2") {
            // Find pattern: `someVar ^ 2` where 2 is a literal (not part of larger number).
            let bytes = optimized.as_bytes();
            let mut i = 0;
            while i + 4 < bytes.len() {
                if &bytes[i..i + 4] == b" ^ 2"
                    && (i + 4 >= bytes.len() || !bytes[i + 4].is_ascii_digit())
                {
                    // Find the start of the operand (scan backwards for ident).
                    let mut start = i;
                    while start > 0 && is_ident_char(bytes[start - 1]) {
                        start -= 1;
                    }
                    if start < i {
                        let operand = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                        if !operand.is_empty() {
                            let replacement = format!("{operand} * {operand}");
                            optimized = format!(
                                "{}{}{}",
                                &optimized[..start],
                                replacement,
                                &optimized[i + 4..]
                            );
                            perf_count += 1;
                            break; // Only one replacement per line to avoid index issues.
                        }
                    }
                }
                i += 1;
            }
        }

        // Note: @native is now emitted directly in emit_function_body() for all
        // user-defined functions (local function form).  The old Pass 6 text-level
        // injection is no longer needed and has been removed to avoid duplicate
        // annotations.

        result.push_str(&optimized);
        result.push('\n');
    }

    if perf_count > 0 {
        *source = result;
        eprintln!("[molt-luau] Applied {} perf optimizations", perf_count);
    }
}

/// Freeze constant tables: when a `local vN = {items}` declaration at indent
/// level 1 (function body top-level) is never mutated, insert `table.freeze(vN)`
/// immediately after the declaration.  The Luau VM optimizes reads from frozen
/// tables and prevents accidental mutation.
/// Spill excess locals to avoid Luau's 200 local register limit.
///
/// For functions with more than 190 local declarations at the top scope,
/// inserts `do...end` blocks every ~180 locals.  Locals inside `do...end`
/// release their registers when the block closes, avoiding the 200 limit.
///
/// The approach: scan each function body, count top-level `local` declarations,
/// and wrap consecutive non-structural lines in `do...end` blocks.
fn spill_excess_locals(source: &mut String) {
    // Insert `do...end` scope blocks inside functions with >180 top-level
    // locals.  Uses a robust indent-based heuristic: top-level lines are
    // those at exactly body_indent.  We count `local` declarations at that
    // indent level and insert scope breaks every 170 locals.
    let lines: Vec<&str> = source.lines().collect();
    let mut result = String::with_capacity(source.len() + 4096);
    let mut total_blocks = 0u32;
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        let is_func_start = !trimmed.starts_with("--")
            && trimmed.contains("= function(")
            && !trimmed.starts_with("local ")
            && (lines[i].starts_with(|c: char| c.is_alphabetic()) || lines[i].starts_with('_'));

        if !is_func_start {
            result.push_str(lines[i]);
            result.push('\n');
            i += 1;
            continue;
        }

        // Emit function header.
        result.push_str(lines[i]);
        result.push('\n');
        i += 1;

        // Determine body indent.
        let body_indent = if i < lines.len() {
            lines[i].len() - lines[i].trim_start().len()
        } else {
            1
        };

        // Pre-scan: count locals at body_indent.
        let mut total_locals = 0u32;
        let mut j = i;
        while j < lines.len() {
            let t = lines[j].trim();
            let ind = lines[j].len() - lines[j].trim_start().len();
            if ind == body_indent && t.starts_with("local ") {
                total_locals += 1;
            }
            // Simple end-of-function detection: `end` at indent 0.
            if ind == 0 && t == "end" {
                break;
            }
            j += 1;
        }
        let func_end = j;

        if total_locals <= 180 {
            continue; // No spilling needed; emit body as-is via the outer loop.
        }

        // Need to spill.  Track indent-based depth relative to body_indent.
        // depth=0 means we're at the function's top scope.
        let indent_str = &lines[i][..body_indent.min(lines[i].len())];
        let mut scope_locals = 0u32;
        let mut in_do = false;

        while i < lines.len() {
            let t = lines[i].trim();
            let ind = lines[i].len() - lines[i].trim_start().len();

            // End of function?
            if i >= func_end {
                if in_do {
                    result.push_str(indent_str);
                    result.push_str("end\n");
                    total_blocks += 1;
                }
                result.push_str(lines[i]);
                result.push('\n');
                i += 1;
                break;
            }

            let at_body_indent = ind == body_indent;

            // Count locals at body_indent.
            if at_body_indent && t.starts_with("local ") {
                scope_locals += 1;
            }

            // Only split at body_indent on simple assignment/local lines
            // (not structural openers like if/for/while).
            let is_safe_split_point = at_body_indent
                && !t.starts_with("if ")
                && !t.starts_with("for ")
                && !t.starts_with("while ")
                && !t.starts_with("repeat")
                && t != "else"
                && t != "end"
                && t != "end)"
                && !t.starts_with("elseif ")
                && !t.starts_with("--");

            // Split when we've accumulated enough locals.
            if scope_locals >= 50 && in_do && is_safe_split_point {
                result.push_str(indent_str);
                result.push_str("end\n");
                result.push_str(indent_str);
                result.push_str("do\n");
                scope_locals = 0;
                total_blocks += 1;
            }

            // Open first do block.
            if !in_do && scope_locals > 0 && is_safe_split_point {
                result.push_str(indent_str);
                result.push_str("do\n");
                in_do = true;
            }

            result.push_str(lines[i]);
            result.push('\n');
            i += 1;
        }
    }

    if total_blocks > 0 {
        eprintln!(
            "[molt-luau] Inserted {} do/end spill blocks for local register limit",
            total_blocks
        );
        *source = result;
    }
}
