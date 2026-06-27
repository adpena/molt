use super::source_text::{
    contains_whole_word_var, has_top_level_binary_op, is_ident_char, is_simple_literal,
    is_simple_var_ref, replace_whole_word,
};
use std::collections::{BTreeMap, BTreeSet};

/// Post-processing pass: inline single-use constants.
///
/// Finds patterns like:
///   local v42 = <literal>
/// where v42 appears exactly once more in the source, and replaces
/// that single use with the literal value, removing the declaration.
pub(super) fn inline_single_use_constants(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: Identify constant declarations and count variable uses.
    let mut const_decls: BTreeMap<String, (usize, String)> = BTreeMap::new(); // var -> (line_idx, rhs)
    let mut var_use_count: BTreeMap<String, usize> = BTreeMap::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Match "local vNNN = <literal>" or "local vNNN: type = <literal>"
        if let Some(rest) = trimmed.strip_prefix("local v")
            && let Some(eq_pos) = rest.find(" = ")
        {
            let before_eq = &rest[..eq_pos];
            // Strip optional type annotation (": number", ": string", etc.)
            let var_suffix = if let Some(colon) = before_eq.find(':') {
                &before_eq[..colon]
            } else {
                before_eq
            };
            if var_suffix.chars().all(|c| c.is_ascii_digit()) {
                let var_name = format!("v{var_suffix}");
                let rhs = rest[eq_pos + 3..].to_string();
                // Only inline simple literals — variable copies are unsafe
                // because the source variable may be reassigned between
                // declaration and use (closure save/restore patterns).
                if is_simple_literal(&rhs) {
                    const_decls.insert(var_name, (i, rhs));
                }
            }
        }

        // Count all vNNN references in this line.
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                    let var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                    *var_use_count.entry(var.to_string()).or_insert(0) += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    // Phase 2: Find single-use constants (declared once + used once = count 2).
    let mut inline_map: BTreeMap<String, String> = BTreeMap::new();
    let mut remove_lines: BTreeSet<usize> = BTreeSet::new();

    for (var, (line_idx, rhs)) in &const_decls {
        if var_use_count.get(var).copied().unwrap_or(0) == 2 {
            // Exactly 2 occurrences: 1 declaration + 1 use.
            // Only inline short literals to avoid code bloat.
            if rhs.len() <= 80 {
                inline_map.insert(var.clone(), rhs.clone());
                remove_lines.insert(*line_idx);
            }
        }
    }

    if inline_map.is_empty() {
        return;
    }

    // Phase 3: Rebuild source with inlining.
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue; // Skip the declaration line.
        }
        let mut new_line = (*line).to_string();
        // Replace variable references with their literal values.
        for (var, literal) in &inline_map {
            if new_line.contains(var.as_str()) {
                new_line = replace_whole_word(&new_line, var, literal);
            }
        }
        result.push_str(&new_line);
        result.push('\n');
    }

    *source = result;
    eprintln!(
        "[molt-luau] Inlined {} single-use constants",
        inline_map.len()
    );
}

/// Remove trailing `continue` statements from loop bodies.
/// `continue` right before `end` in a loop is a no-op — the loop naturally
/// continues to the next iteration at `end`.

pub(super) fn strip_undefined_rhs_assignments(source: &mut String) {
    use std::collections::BTreeSet;

    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: Collect all defined variables (declared or assigned to).
    let mut defined_vars: BTreeSet<String> = BTreeSet::new();
    for line in &lines {
        let trimmed = line.trim();
        // `local vN` or `local vN = ...`
        if let Some(rest) = trimmed.strip_prefix("local ") {
            let var_end = rest
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(rest.len());
            let var = &rest[..var_end];
            if !var.is_empty() {
                defined_vars.insert(var.to_string());
            }
        }
        // `vN = ...` (assignment, not `local`)
        if trimmed.starts_with('v')
            && let Some(eq_pos) = trimmed.find(" = ")
        {
            let lhs = &trimmed[..eq_pos];
            if lhs.starts_with('v') && lhs[1..].chars().all(|c| c.is_ascii_digit()) {
                defined_vars.insert(lhs.to_string());
            }
        }
    }
    // Function parameters are also defined.
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.ends_with(')')
            && (trimmed.contains("= function(") || trimmed.contains("function "))
            && let Some(paren_start) = trimmed.rfind('(')
        {
            let params = &trimmed[paren_start + 1..trimmed.len() - 1];
            for param in params.split(", ") {
                let p = param.trim();
                if !p.is_empty() {
                    defined_vars.insert(p.to_string());
                }
            }
        }
        // For-loop iteration variables: `for _, vN in ...` or `for vN = ...`
        if let Some(rest) = trimmed.strip_prefix("for ") {
            // Split on " in " or " = " to get the variable list
            let var_part = if let Some(in_pos) = rest.find(" in ") {
                &rest[..in_pos]
            } else if let Some(eq_pos) = rest.find(" = ") {
                &rest[..eq_pos]
            } else {
                continue;
            };
            for var in var_part.split(", ") {
                let v = var.trim();
                if !v.is_empty() && v != "_" {
                    defined_vars.insert(v.to_string());
                }
            }
        }
    }

    // Phase 2: Find `vN = vM` lines where vM is NOT in defined_vars.
    let mut remove: BTreeSet<usize> = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // Match: `vN = vM` (bare assignment, not `local`)
        if !trimmed.starts_with("local ")
            && trimmed.starts_with('v')
            && let Some(eq_pos) = trimmed.find(" = ")
        {
            let lhs = &trimmed[..eq_pos];
            let rhs = trimmed[eq_pos + 3..].trim();
            // Both sides must be simple variable names (vN pattern).
            if lhs.starts_with('v')
                && lhs[1..].chars().all(|c| c.is_ascii_digit())
                && rhs.starts_with('v')
                && rhs[1..].chars().all(|c| c.is_ascii_digit())
                && !defined_vars.contains(rhs)
            {
                remove.insert(i);
            }
        }
    }

    if remove.is_empty() {
        return;
    }

    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if !remove.contains(&i) {
            result.push_str(line);
            result.push('\n');
        }
    }
    *source = result;
    eprintln!(
        "[molt-luau] Stripped {} dead undefined-RHS assignments",
        remove.len()
    );
}

/// Propagate single-use variable copies: `local vN = vM` where vN is used
/// exactly once → replace vN with vM at the use site and remove the declaration.
/// Only applies when vM is not reassigned between declaration and use.
///
/// Runs up to 3 iterations to collapse chains (vA → vB → vC).
pub(super) fn propagate_single_use_copies(source: &mut String) {
    let mut total = 0;
    for _ in 0..3 {
        let count = propagate_single_use_copies_once(source);
        if count == 0 {
            break;
        }
        total += count;
    }
    if total > 0 {
        eprintln!("[molt-luau] Propagated {} single-use copies", total);
    }
}

fn propagate_single_use_copies_once(source: &mut String) -> usize {
    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: Find `local vN = vM` (or `local vN: type = vM`) copy
    // declarations and count all var uses.
    let mut copy_decls: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut var_use_count: BTreeMap<String, usize> = BTreeMap::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("local v")
            && let Some(eq_pos) = rest.find(" = ")
        {
            let before_eq = &rest[..eq_pos];
            // Strip optional type annotation (": number", etc.)
            let var_suffix = if let Some(colon) = before_eq.find(':') {
                &before_eq[..colon]
            } else {
                before_eq
            };
            if var_suffix.chars().all(|c| c.is_ascii_digit()) {
                let var_name = format!("v{var_suffix}");
                let rhs = rest[eq_pos + 3..].trim();
                if is_simple_var_ref(rhs) {
                    copy_decls.insert(var_name, (i, rhs.to_string()));
                }
            }
        }

        // Count all vNNN references in this line.
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                    let var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                    *var_use_count.entry(var.to_string()).or_insert(0) += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    // Phase 2: For single-use copies, verify source is not reassigned between
    // declaration and use.
    let mut inline_map: BTreeMap<String, String> = BTreeMap::new();
    let mut remove_lines: BTreeSet<usize> = BTreeSet::new();

    for (var, (decl_line, source_var)) in &copy_decls {
        let count = var_use_count.get(var).copied().unwrap_or(0);
        if count != 2 {
            continue; // 1 decl + 1 use = 2
        }

        // Find the use line.
        let mut use_idx = None;
        for (i, line) in lines.iter().enumerate() {
            if i == *decl_line {
                continue;
            }
            if contains_whole_word_var(line, var) {
                use_idx = Some(i);
                break;
            }
        }

        let Some(use_line) = use_idx else { continue };
        if use_line <= *decl_line {
            continue; // Use before decl — skip.
        }

        // Verify source_var is not reassigned between decl and use.
        let mut reassigned = false;
        for i in (*decl_line + 1)..use_line {
            let t = lines[i].trim();
            if t.starts_with("--") {
                continue;
            }
            // Bare assignment: `source_var = ...`
            let assign_pat = format!("{source_var} = ");
            if t.starts_with(&assign_pat) {
                reassigned = true;
                break;
            }
        }

        if !reassigned {
            inline_map.insert(var.clone(), source_var.clone());
            remove_lines.insert(*decl_line);
        }
    }

    if inline_map.is_empty() {
        return 0;
    }

    let count = inline_map.len();
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue;
        }
        let mut new_line = (*line).to_string();
        for (var, replacement) in &inline_map {
            if new_line.contains(var.as_str()) {
                new_line = replace_whole_word(&new_line, var, replacement);
            }
        }
        result.push_str(&new_line);
        result.push('\n');
    }

    *source = result;
    count
}

/// Check if a string is a simple variable reference (v\d+ or parameter name).

/// Simplify return chains where a hoisted variable is assigned just before return.
/// Pattern: `vN = expr; [comment lines]; return vN` → `return expr`
/// Only applies when vN is used exactly 3 times total (decl + assign + return).
pub(super) fn simplify_return_chain(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();

    // Count all variable uses.
    let mut var_use_count: BTreeMap<String, usize> = BTreeMap::new();
    for line in &lines {
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                    let var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                    *var_use_count.entry(var.to_string()).or_insert(0) += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    let mut remove_lines: BTreeSet<usize> = BTreeSet::new();
    let mut replacements: BTreeMap<usize, String> = BTreeMap::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Match `return vN`
        if let Some(ret_var) = trimmed.strip_prefix("return ") {
            let ret_var = ret_var.trim();
            if !ret_var.starts_with('v')
                || ret_var.len() < 2
                || !ret_var[1..].chars().all(|c| c.is_ascii_digit())
            {
                continue;
            }

            // Scan backwards for `vN = expr`, skipping comments and blank lines.
            let mut assign_line = None;
            let mut assign_rhs = None;
            let mut j = i.wrapping_sub(1);
            while j < lines.len() {
                let prev = lines[j].trim();
                if prev.is_empty() || prev.starts_with("--") {
                    if j == 0 {
                        break;
                    }
                    j -= 1;
                    continue;
                }
                // Match `vN = expr` (bare assignment, not local)
                let assign_pat = format!("{ret_var} = ");
                if let Some(rhs) = prev.strip_prefix(&assign_pat) {
                    // Verify this is a bare assignment, not inside an if/etc.
                    assign_line = Some(j);
                    assign_rhs = Some(rhs.to_string());
                }
                break;
            }

            if let (Some(a_line), Some(rhs)) = (assign_line, assign_rhs) {
                // Check that ret_var has exactly 3 uses (decl + assign + return).
                let count = var_use_count.get(ret_var).copied().unwrap_or(0);
                if count == 3 {
                    let indent = &line[..line.len() - trimmed.len()];
                    replacements.insert(i, format!("{indent}return {rhs}"));
                    remove_lines.insert(a_line);
                }
            }
        }
    }

    if remove_lines.is_empty() && replacements.is_empty() {
        return;
    }

    let count = replacements.len();
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue;
        }
        if let Some(replacement) = replacements.get(&i) {
            result.push_str(replacement);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    *source = result;
    eprintln!("[molt-luau] Simplified {} return chains", count);
}

/// Returns `true` if the expression is a *pure* non-trivial RHS suitable for
/// common-subexpression elimination or loop-invariant hoisting.
///
/// "Pure" means no observable side effects: arithmetic, comparisons,
/// table reads, known-pure math/string builtins, concatenation, and the
/// length operator are accepted.  Arbitrary function calls are rejected.

/// Sink single-use locals into their sole consumer when the consumer is on
/// the immediately following (non-blank, non-comment) line.
///
/// `local vN = <expr>` followed by a line that uses vN exactly once →
/// remove the local declaration, replace vN with `<expr>` inline.
/// Only applies when the expression is ≤120 chars (avoids line bloat).
///
/// Runs iteratively to handle chains (vA → vB → vC) without introducing
/// dangling references.
pub(super) fn sink_single_use_locals(source: &mut String) {
    let mut total = 0;
    for _ in 0..5 {
        let count = sink_single_use_locals_once(source);
        if count == 0 {
            break;
        }
        total += count;
    }
    if total > 0 {
        eprintln!(
            "[molt-luau] Sunk {} single-use locals into next line",
            total
        );
    }
}

fn sink_single_use_locals_once(source: &mut String) -> usize {
    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: Find all `local vN = <expr>` (or typed `local vN: type = <expr>`)
    // and count uses.
    let mut local_decls: BTreeMap<String, (usize, String)> = BTreeMap::new();
    let mut var_use_count: BTreeMap<String, usize> = BTreeMap::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("local v")
            && let Some(eq_pos) = rest.find(" = ")
        {
            let before_eq = &rest[..eq_pos];
            // Strip optional type annotation (": number", etc.)
            let var_suffix = if let Some(colon) = before_eq.find(':') {
                &before_eq[..colon]
            } else {
                before_eq
            };
            if var_suffix.chars().all(|c| c.is_ascii_digit()) {
                let var_name = format!("v{var_suffix}");
                let rhs = rest[eq_pos + 3..].trim().to_string();
                if rhs.len() <= 120
                    && !rhs.contains('\n')
                    && !rhs.contains("--")
                    && !is_simple_literal(&rhs)
                    && !is_simple_var_ref(&rhs)
                {
                    local_decls.insert(var_name, (i, rhs));
                }
            }
        }

        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                    let var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                    *var_use_count.entry(var.to_string()).or_insert(0) += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    // Phase 2: Collect candidates. Skip if the RHS references another variable
    // that is ALSO a candidate (chain hazard — handle in the next iteration).
    let mut candidates: BTreeMap<String, (usize, String)> = BTreeMap::new();
    for (var, (decl_line, expr)) in &local_decls {
        let count = var_use_count.get(var).copied().unwrap_or(0);
        if count != 2 {
            continue;
        }

        // Find the next non-blank, non-comment line.
        let mut next_line = *decl_line + 1;
        while next_line < lines.len() {
            let t = lines[next_line].trim();
            if !t.is_empty() && !t.starts_with("--") {
                break;
            }
            next_line += 1;
        }
        if next_line >= lines.len() {
            continue;
        }

        if contains_whole_word_var(lines[next_line], var) {
            candidates.insert(var.clone(), (*decl_line, expr.clone()));
        }
    }

    // Filter out candidates whose RHS references another candidate variable.
    let candidate_vars: BTreeSet<String> = candidates.keys().cloned().collect();
    let mut inline_map: BTreeMap<String, String> = BTreeMap::new();
    let mut remove_lines: BTreeSet<usize> = BTreeSet::new();

    for (var, (decl_line, expr)) in &candidates {
        let rhs_references_candidate = candidate_vars
            .iter()
            .any(|other| other != var && contains_whole_word_var(expr, other));
        if !rhs_references_candidate {
            // Wrap in parentheses when needed for correctness:
            // - Table constructors: `{...}[n]` is a Luau syntax error
            // - Top-level binary operators: inlining `a + b` into `expr * 2`
            //   would change precedence without parens
            let safe_expr = if expr.starts_with('{') || has_top_level_binary_op(expr) {
                format!("({expr})")
            } else {
                expr.clone()
            };
            inline_map.insert(var.clone(), safe_expr);
            remove_lines.insert(*decl_line);
        }
    }

    if inline_map.is_empty() {
        return 0;
    }

    let count = inline_map.len();
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue;
        }
        let mut new_line = (*line).to_string();
        for (var, replacement) in &inline_map {
            if new_line.contains(var.as_str()) {
                new_line = replace_whole_word(&new_line, var, replacement);
            }
        }
        result.push_str(&new_line);
        result.push('\n');
    }

    *source = result;
    count
}

/// Multi-return optimization: replace `local vN = {a, b, c}; return table.unpack(vN)`
/// with `return a, b, c`, eliminating an unnecessary table allocation.
pub(super) fn optimize_multi_return(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();
    let mut remove_lines: BTreeSet<usize> = BTreeSet::new();
    let mut replacements: BTreeMap<usize, String> = BTreeMap::new();

    // Count variable uses for filtering.
    let mut var_use_count: BTreeMap<String, usize> = BTreeMap::new();
    for line in &lines {
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' && (pos == 0 || !is_ident_char(bytes[pos - 1])) {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 && (pos >= bytes.len() || !is_ident_char(bytes[pos])) {
                    let var = std::str::from_utf8(&bytes[start..pos]).unwrap_or("");
                    *var_use_count.entry(var.to_string()).or_insert(0) += 1;
                }
            } else {
                pos += 1;
            }
        }
    }

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // Match: `local vN = {items}`
        if !trimmed.starts_with("local v") {
            continue;
        }
        let rest = &trimmed["local ".len()..];
        let var_end = rest
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        let var_name = &rest[..var_end];
        if !var_name.starts_with('v')
            || var_name.len() < 2
            || !var_name[1..].chars().all(|c| c.is_ascii_digit())
        {
            continue;
        }
        let after_var = rest[var_end..].trim();
        if !after_var.starts_with("= {") || !after_var.ends_with('}') {
            continue;
        }

        // Extract items between { and }
        let inner = &after_var[2..after_var.len()].trim();
        let inner = &inner[1..inner.len() - 1]; // strip { and }
        // Must be positional entries only (no `=` signs which indicate keyed entries)
        if inner.contains('=') {
            continue;
        }
        let items_str = inner.trim();
        if items_str.is_empty() {
            continue;
        }

        // vN must be used exactly 2 times (declaration + return)
        let count = var_use_count.get(var_name).copied().unwrap_or(0);
        if count != 2 {
            continue;
        }

        // Look for `return table.unpack(vN)` on a following line
        let expected_return = format!("return table.unpack({var_name})");
        let mut found_return = None;
        for j in (i + 1)..lines.len() {
            let jt = lines[j].trim();
            if jt.is_empty() || jt.starts_with("--") {
                continue;
            }
            if jt == expected_return {
                found_return = Some(j);
            }
            break;
        }

        if let Some(ret_line) = found_return {
            let indent = &line[..line.len() - trimmed.len()];
            replacements.insert(ret_line, format!("{indent}return {items_str}"));
            remove_lines.insert(i);
        }
    }

    if remove_lines.is_empty() && replacements.is_empty() {
        return;
    }

    let count = replacements.len();
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue;
        }
        if let Some(replacement) = replacements.get(&i) {
            result.push_str(replacement);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    *source = result;
    eprintln!(
        "[molt-luau] Optimized {} multi-return pack/unpack sequences",
        count
    );
}
