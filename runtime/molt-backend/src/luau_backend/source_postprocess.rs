use std::collections::{BTreeMap, BTreeSet};
#[path = "source_postprocess/control_flow.rs"]
mod control_flow;
use control_flow::{
    eliminate_goto_labels, rehoist_escaped_locals, strip_dead_code_after_terminators,
    strip_dead_gotos_and_labels, strip_exception_cleanup_blocks, structure_forward_if_else_blocks,
    structure_pcall_failure_blocks,
};

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

/// Post-processing pass: inline single-use constants.
///
/// Finds patterns like:
///   local v42 = <literal>
/// where v42 appears exactly once more in the source, and replaces
/// that single use with the literal value, removing the declaration.
fn inline_single_use_constants(source: &mut String) {
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

fn is_simple_literal(s: &str) -> bool {
    if s == "nil" || s == "true" || s == "false" {
        return true;
    }
    // Numeric: optional minus, digits, optional decimal
    let bytes = s.as_bytes();
    if !bytes.is_empty() {
        let start = if bytes[0] == b'-' { 1 } else { 0 };
        if start < bytes.len() && bytes[start].is_ascii_digit() {
            return bytes[start..]
                .iter()
                .all(|&b| b.is_ascii_digit() || b == b'.');
        }
    }
    // String: starts and ends with "
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        return true;
    }
    false
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_ident_char_scalar(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Replace whole-word occurrences of `needle` with `replacement` in `haystack`.
fn replace_whole_word(haystack: &str, needle: &str, replacement: &str) -> String {
    let mut result = String::with_capacity(haystack.len() + replacement.len());
    let mut last = 0;

    for (pos, _) in haystack.match_indices(needle) {
        let end = pos + needle.len();
        let before_ok = haystack[..pos]
            .chars()
            .next_back()
            .is_none_or(|c| !is_ident_char_scalar(c));
        let after_ok = haystack[end..]
            .chars()
            .next()
            .is_none_or(|c| !is_ident_char_scalar(c));
        if !(before_ok && after_ok) {
            continue;
        }

        result.push_str(&haystack[last..pos]);
        // Don't replace at declaration positions with literals — `local vN`
        // should never become `local "string"` or `local 42`.
        let is_decl_pos = haystack[..pos].ends_with("local ");
        let replacement_is_literal = replacement.starts_with('"')
            || replacement.starts_with('{')
            || replacement == "nil"
            || replacement == "true"
            || replacement == "false"
            || replacement.starts_with(|c: char| c.is_ascii_digit())
            || replacement.starts_with('-');
        if is_decl_pos && replacement_is_literal {
            result.push_str(&haystack[pos..end]);
        } else {
            result.push_str(replacement);
        }
        last = end;
    }
    result.push_str(&haystack[last..]);
    result
}

/// Eliminate `local vN = nil -- [missing]` / `local vM = {vN}` pairs.
///
/// These arise from Python's default-argument mechanism: the IR creates
/// a `missing` sentinel wrapped in a single-element callargs table.
/// When the nil variable is only used in the wrapper, we can replace the
/// wrapper with `{nil}` and remove the nil declaration entirely.
fn eliminate_nil_missing_wrappers(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();
    let mut var_use_count: BTreeMap<String, usize> = BTreeMap::new();

    // Count uses of all vNNN variables.
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

    // Find lines matching `local vN = nil -- [missing]` where vN has exactly
    // 2 uses (declaration + one wrapper).  Mark the line for removal and record
    // the variable for replacement in the wrapper line.
    let mut remove_lines: BTreeSet<usize> = BTreeSet::new();
    let mut nil_vars: BTreeSet<String> = BTreeSet::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("local v")
            && let Some(suffix) = rest.strip_suffix(" = nil -- [missing]")
            && suffix.chars().all(|c| c.is_ascii_digit())
        {
            let var = format!("v{suffix}");
            if var_use_count.get(&var).copied().unwrap_or(0) == 2 {
                remove_lines.insert(i);
                nil_vars.insert(var);
            }
        }
    }

    if nil_vars.is_empty() {
        return;
    }

    // Rebuild source, replacing `{vN}` with `{nil}` for eliminated vars.
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove_lines.contains(&i) {
            continue;
        }
        let mut new_line = (*line).to_string();
        for var in &nil_vars {
            let wrapper = format!("{{{var}}}");
            if new_line.contains(&wrapper) {
                new_line = new_line.replace(&wrapper, "{nil}");
            }
        }
        result.push_str(&new_line);
        result.push('\n');
    }

    let removed = remove_lines.len();
    *source = result;
    eprintln!("[molt-luau] Eliminated {} nil-missing wrappers", removed);
}

/// Strip dead UnboundLocalError checks.
///
/// Pattern (3-5 lines):
///   local vN = vM == vP       (comparison — vP is often undefined)
///   if vN then
///       local vQ = "cannot access local variable ..."
///       local vR = "UnboundLocalError"
///   end
///
/// These are Python unbound-variable guards that can never trigger in
/// transpiled Luau (all variables are initialized). Remove the entire block.
fn strip_unbound_local_checks(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();
    let mut remove: BTreeSet<usize> = BTreeSet::new();
    let len = lines.len();

    let mut i = 0;
    while i < len {
        let trimmed = lines[i].trim();
        // Match: `if vN then`
        if trimmed.starts_with("if v") && trimmed.ends_with(" then") {
            // Look ahead: next line should be a string containing "cannot access local variable"
            if i + 1 < len && lines[i + 1].contains("cannot access local variable") {
                // Find the closing `end`
                let mut j = i + 2;
                while j < len {
                    if lines[j].trim() == "end" {
                        break;
                    }
                    j += 1;
                }
                if j < len && lines[j].trim() == "end" {
                    // Also remove the comparison line before the `if`
                    // Pattern: `local vN = vM == vP`
                    if i > 0 {
                        let prev = lines[i - 1].trim();
                        if prev.starts_with("local v") && prev.contains(" == ") {
                            remove.insert(i - 1);
                        }
                    }
                    for k in i..=j {
                        remove.insert(k);
                    }
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
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
        "[molt-luau] Stripped {} UnboundLocalError check lines",
        remove.len()
    );
}

/// Strip dead locals-dict stores.
///
/// Pattern: `vN["name"] = expr` where vN is a locals dict (created as `{}`
/// and only used for store/module metadata). These are Python frame
/// introspection writes — the dict is never read in transpiled Luau.
///
/// We detect the locals dict by looking for the pattern:
///   `local vN = {}` (empty table) followed only by bracket-store writes.
fn strip_dead_locals_dict_stores(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();

    // Phase 1: Find candidates — `local vN = {}` or `local vN: type = {}`
    let mut candidates: BTreeMap<String, usize> = BTreeMap::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("local v")
            && rest.ends_with(" = {}")
            && let Some(eq_pos) = rest.find(" = {}")
        {
            let before_eq = &rest[..eq_pos];
            let suffix = if let Some(colon) = before_eq.find(':') {
                &before_eq[..colon]
            } else {
                before_eq
            };
            if suffix.chars().all(|c| c.is_ascii_digit()) {
                let var = format!("v{suffix}");
                candidates.insert(var, i);
            }
        }
    }

    if candidates.is_empty() {
        return;
    }

    // Phase 2: For each candidate, check if it's ONLY used as:
    //   vN["name"] = expr  (store)
    //   if type(vN) == "table" then vN["name"] = expr end  (guarded store)
    // If it's read in any other context, it's live — skip.
    let mut dead_dicts: BTreeSet<String> = BTreeSet::new();

    for var in candidates.keys() {
        let var_bytes = var.as_bytes();
        let mut is_dead = true;

        for line in &lines {
            let trimmed = line.trim();
            let bytes = trimmed.as_bytes();
            // Find all occurrences of var in this line
            let mut pos = 0;
            while pos + var_bytes.len() <= bytes.len() {
                if &bytes[pos..pos + var_bytes.len()] == var_bytes {
                    let before_ok = pos == 0 || !is_ident_char(bytes[pos - 1]);
                    let after_ok = pos + var_bytes.len() >= bytes.len()
                        || !is_ident_char(bytes[pos + var_bytes.len()]);
                    if before_ok && after_ok {
                        // Check context: is this a declaration, store, guarded store,
                        // or type-check guard part of a guarded store line?
                        let is_decl = trimmed.starts_with(&format!("local {var} = {{}}"))
                            || trimmed.starts_with(&format!("local {var}: "))
                                && trimmed.ends_with(" = {}");
                        let is_store = {
                            let after = &trimmed[pos + var_bytes.len()..];
                            after.starts_with("[\"")
                        };
                        // Accept type(vN) on a guarded-store line
                        let is_type_check = pos >= 5 && &trimmed[pos - 5..pos] == "type(";
                        let on_guarded_line = trimmed.starts_with("if type(")
                            && trimmed.contains(&format!("{var}[\""));
                        if !(is_decl || is_store || (is_type_check && on_guarded_line)) {
                            is_dead = false;
                            break;
                        }
                    }
                }
                pos += 1;
            }
            if !is_dead {
                break;
            }
        }

        if is_dead {
            dead_dicts.insert(var.clone());
        }
    }

    if dead_dicts.is_empty() {
        return;
    }

    // Phase 3: Remove declaration lines and all store lines referencing dead dicts.
    let mut remove: BTreeSet<usize> = BTreeSet::new();
    for var in &dead_dicts {
        if let Some(&decl_line) = candidates.get(var) {
            remove.insert(decl_line);
        }
    }

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        for var in &dead_dicts {
            // Direct store: `vN["name"] = expr`
            if trimmed.starts_with(&format!("{var}[\"")) {
                remove.insert(i);
                break;
            }
            // Guarded store: `if type(vN) == "table" then vN["name"] = expr end`
            if trimmed.starts_with(&format!("if type({var})"))
                && trimmed.contains(&format!("{var}[\""))
            {
                remove.insert(i);
                break;
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
    let total = remove.len();
    *source = result;
    eprintln!(
        "[molt-luau] Stripped {} dead locals-dict lines ({} dicts)",
        total,
        dead_dicts.len()
    );
}

/// Remove trailing `continue` statements from loop bodies.
/// `continue` right before `end` in a loop is a no-op — the loop naturally
/// continues to the next iteration at `end`.
fn strip_trailing_continue(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();
    let mut remove: BTreeSet<usize> = BTreeSet::new();

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed == "continue" {
            // Check if next non-blank line is `end`
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            if j < lines.len() && lines[j].trim() == "end" {
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
        "[molt-luau] Stripped {} trailing continue statements",
        remove.len()
    );
}

/// Simplify comparison-break patterns in while-true loops.
/// `local vN = vA < vB; if not vN then break end` → `if vA >= vB then break end`
fn simplify_comparison_break(source: &mut String) {
    use std::collections::{BTreeMap, BTreeSet};
    let lines: Vec<&str> = source.lines().collect();
    let mut remove: BTreeSet<usize> = BTreeSet::new();
    let mut replacements: BTreeMap<usize, String> = BTreeMap::new();

    for i in 0..lines.len().saturating_sub(1) {
        let trimmed = lines[i].trim();
        let next_trimmed = lines[i + 1].trim();

        // Match: `local vN = vA < vB`
        if let Some(rest) = trimmed.strip_prefix("local v")
            && let Some(eq_pos) = rest.find(" = ")
        {
            let var_suffix = &rest[..eq_pos];
            if !var_suffix.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let var_name = format!("v{var_suffix}");
            let rhs = &rest[eq_pos + 3..];

            // Check if next line is `if not vN then break end`
            let expected_if = format!("if not {var_name} then break end");
            if next_trimmed != expected_if {
                continue;
            }

            // Try to find comparison op in rhs
            let (lhs, op, rhs_val) = if let Some(pos) = rhs.find(" < ") {
                (&rhs[..pos], ">=", &rhs[pos + 3..])
            } else if let Some(pos) = rhs.find(" > ") {
                (&rhs[..pos], "<=", &rhs[pos + 3..])
            } else if let Some(pos) = rhs.find(" <= ") {
                (&rhs[..pos], ">", &rhs[pos + 4..])
            } else if let Some(pos) = rhs.find(" >= ") {
                (&rhs[..pos], "<", &rhs[pos + 4..])
            } else if let Some(pos) = rhs.find(" == ") {
                (&rhs[..pos], "~=", &rhs[pos + 4..])
            } else if let Some(pos) = rhs.find(" ~= ") {
                (&rhs[..pos], "==", &rhs[pos + 4..])
            } else {
                continue;
            };

            // Verify var_name is only used on these 2 lines
            let var_bytes = var_name.as_bytes();
            let mut total_uses = 0;
            for line in &lines {
                let bytes = line.as_bytes();
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
            // 1 in decl + 1 in if = 2
            if total_uses != 2 {
                continue;
            }

            let indent = &lines[i][..lines[i].len() - trimmed.len()];
            replacements.insert(i, format!("{indent}if {lhs} {op} {rhs_val} then break end"));
            remove.insert(i + 1);
        }
    }

    if remove.is_empty() && replacements.is_empty() {
        return;
    }

    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if remove.contains(&i) {
            continue;
        }
        if let Some(replacement) = replacements.get(&i) {
            result.push_str(replacement);
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    let count = remove.len() + replacements.len();
    *source = result;
    eprintln!(
        "[molt-luau] Simplified {} comparison-break patterns",
        count / 2
    );
}

/// Eliminate assignments where the RHS variable is never declared or assigned
/// anywhere in the function body. These are dead closure-restore ops: the
/// frontend emits frame-restore writes (`vN = vM`) where `vM` was a closure
/// cell that got stripped by tree_shake_luau. In Luau, reading an undeclared
/// local yields nil, making these assignments dead writes.
fn strip_undefined_rhs_assignments(source: &mut String) {
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
fn propagate_single_use_copies(source: &mut String) {
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
fn is_simple_var_ref(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // v\d+ pattern
    if s.starts_with('v') && s.len() > 1 && s[1..].chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // Simple parameter names (alphabetic + underscore, no dots/brackets/parens)
    if s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        // Exclude Luau keywords
        !matches!(
            s,
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
        )
    } else {
        false
    }
}

/// Check if `s` matches the Molt IR variable pattern `v\d+`.
#[allow(dead_code)]
fn is_molt_var(s: &str) -> bool {
    s.starts_with('v') && s.len() > 1 && s[1..].chars().all(|c| c.is_ascii_digit())
}

/// Scan source lines and count whole-word references to `v\d+` variables.
/// Returns a map from variable name → reference count.
#[allow(dead_code)]
fn count_var_uses(lines: &[&str]) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for line in lines {
        let bytes = line.as_bytes();
        let mut pos = 0;
        while pos < bytes.len() {
            if bytes[pos] == b'v' {
                let start = pos;
                pos += 1;
                while pos < bytes.len() && bytes[pos].is_ascii_digit() {
                    pos += 1;
                }
                if pos > start + 1 {
                    // Check word boundaries.
                    let left_ok = start == 0
                        || !bytes[start - 1].is_ascii_alphanumeric() && bytes[start - 1] != b'_';
                    let right_ok = pos >= bytes.len()
                        || !bytes[pos].is_ascii_alphanumeric() && bytes[pos] != b'_';
                    if left_ok && right_ok {
                        let var = &line[start..pos];
                        *counts.entry(var.to_string()).or_insert(0) += 1;
                    }
                }
            } else {
                pos += 1;
            }
        }
    }
    counts
}

/// Check if `line` contains a whole-word occurrence of `var`.
fn contains_whole_word_var(line: &str, var: &str) -> bool {
    let bytes = line.as_bytes();
    let var_bytes = var.as_bytes();
    let mut pos = 0;
    while pos + var_bytes.len() <= bytes.len() {
        if &bytes[pos..pos + var_bytes.len()] == var_bytes {
            let before_ok = pos == 0 || !is_ident_char(bytes[pos - 1]);
            let after_ok = pos + var_bytes.len() >= bytes.len()
                || !is_ident_char(bytes[pos + var_bytes.len()]);
            if before_ok && after_ok {
                return true;
            }
        }
        pos += 1;
    }
    false
}

/// Simplify return chains where a hoisted variable is assigned just before return.
/// Pattern: `vN = expr; [comment lines]; return vN` → `return expr`
/// Only applies when vN is used exactly 3 times total (decl + assign + return).
fn simplify_return_chain(source: &mut String) {
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
fn is_pure_expr(s: &str) -> bool {
    // Reject simple literals and variable refs — no point in CSE for those.
    if is_simple_literal(s) || is_simple_var_ref(s) {
        return false;
    }
    // Table constructors create NEW mutable objects — CSE would alias them.
    if s.starts_with('{') {
        return false;
    }
    // If the expression contains a parenthesised call, only allow known-pure
    // math/string/conversion functions.
    if s.contains('(') {
        const ALLOWED: &[&str] = &[
            "math.floor(",
            "math.sqrt(",
            "math.abs(",
            "math.sin(",
            "math.cos(",
            "math.ceil(",
            "math_floor(",
            "math.min(",
            "math.max(",
            "string.find(",
            "string.sub(",
            "string.len(",
            "tonumber(",
            "tostring(",
        ];
        if !ALLOWED.iter().any(|p| s.contains(p)) {
            return false;
        }
    }
    // Must not contain an embedded assignment.
    if s.contains(" = ") {
        return false;
    }
    true
}

/// Common-subexpression elimination (CSE).
///
/// Scans for `local vN = <pure_expr>` declarations.  When the *exact* same
/// pure expression appears as the RHS of a later `local vM = <pure_expr>` at
/// the same indentation depth, the second declaration is rewritten to
/// `local vM = vN` (reuse the first computation).
///
/// Only applies when `vN` is not reassigned between the two declarations and
/// none of the variables referenced in the expression are reassigned either.
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

/// Sink single-use locals into their sole consumer when the consumer is on
/// the immediately following (non-blank, non-comment) line.
///
/// `local vN = <expr>` followed by a line that uses vN exactly once →
/// remove the local declaration, replace vN with `<expr>` inline.
/// Only applies when the expression is ≤120 chars (avoids line bloat).
///
/// Runs iteratively to handle chains (vA → vB → vC) without introducing
/// dangling references.
fn sink_single_use_locals(source: &mut String) {
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

/// Find the matching closing parenthesis for an opening paren at `open_pos`.
/// Check if an expression contains binary operators at the top level
/// (not inside `[]`, `()`, or `{}`). Used by the sink pass to decide
/// whether inlined expressions need parenthesization.
fn has_top_level_binary_op(expr: &str) -> bool {
    let bytes = expr.as_bytes();
    let mut depth: i32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'+' | b'-' | b'*' | b'/' | b'%' | b'^' if depth == 0 => {
                // Must be a binary op: preceded and followed by space
                if i > 0 && i + 1 < bytes.len() && bytes[i - 1] == b' ' && bytes[i + 1] == b' ' {
                    return true;
                }
            }
            b'.' if depth == 0 && i + 1 < bytes.len() && bytes[i + 1] == b'.' => {
                return true; // string concatenation `..`
            }
            _ => {}
        }
        i += 1;
    }
    false
}

/// Find the matching closing parenthesis for an opening paren at `open_pos`.
fn find_matching_paren(s: &str, open_pos: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1;
    let mut i = open_pos + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
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

/// Multi-return optimization: replace `local vN = {a, b, c}; return table.unpack(vN)`
/// with `return a, b, c`, eliminating an unnecessary table allocation.
fn optimize_multi_return(source: &mut String) {
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

/// Index folding for range loops: when `for vN = 0, expr - 1 do` and every use
/// of `vN` in the loop body is `[vN + 1]`, rewrite to `for vN = 1, expr do`
/// and replace `[vN + 1]` with `[vN]`, eliminating one ADD per iteration.
fn fold_range_indices(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();
    let mut replacements: BTreeMap<usize, String> = BTreeMap::new();

    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        // Match: `for vN = 0, EXPR - 1 do` or `for vN = 0, EXPR - 1, 1 do`
        if !trimmed.starts_with("for v") {
            i += 1;
            continue;
        }
        let rest = &trimmed["for ".len()..];
        let eq_pos = match rest.find(" = ") {
            Some(p) => p,
            None => {
                i += 1;
                continue;
            }
        };
        let var_name = &rest[..eq_pos];
        if !var_name.starts_with('v')
            || var_name.len() < 2
            || !var_name[1..].chars().all(|c| c.is_ascii_digit())
        {
            i += 1;
            continue;
        }

        let after_eq = rest[eq_pos + 3..].trim(); // after " = "
        // Must start with "0, "
        if !after_eq.starts_with("0, ") {
            i += 1;
            continue;
        }
        let bound_and_rest = &after_eq[3..]; // after "0, "

        // Must end with " do"
        if !bound_and_rest.ends_with(" do") {
            i += 1;
            continue;
        }
        let bound_part = &bound_and_rest[..bound_and_rest.len() - 3]; // strip " do"

        // Strip optional ", 1" step suffix
        let bound_expr = bound_part.strip_suffix(", 1").unwrap_or(bound_part);

        // Must end with " - 1"
        if !bound_expr.ends_with(" - 1") {
            i += 1;
            continue;
        }
        let upper_expr = &bound_expr[..bound_expr.len() - 4]; // the EXPR part

        // Find loop body: from i+1 to matching `end`
        let loop_indent = &lines[i][..lines[i].len() - trimmed.len()];
        let mut depth = 1i32;
        let loop_start = i + 1;
        let mut loop_end = None;
        for j in loop_start..lines.len() {
            let jt = lines[j].trim();
            // Count block openers/closers
            if jt.starts_with("for ")
                || jt.starts_with("while ")
                || jt.starts_with("if ")
                || jt == "repeat"
                || (jt.starts_with("function ") && jt.ends_with(")"))
            {
                // Only count if it opens a block (ends with "do", "then", or ")")
                if jt.ends_with(" do")
                    || jt.ends_with(" then")
                    || jt == "repeat"
                    || jt.ends_with(")")
                {
                    depth += 1;
                }
            }
            if jt == "end" {
                depth -= 1;
                if depth == 0 {
                    // Verify it's at the same indent level
                    let j_indent = &lines[j][..lines[j].len() - jt.len()];
                    if j_indent == loop_indent {
                        loop_end = Some(j);
                    }
                    break;
                }
            }
        }

        let loop_end = match loop_end {
            Some(e) => e,
            None => {
                i += 1;
                continue;
            }
        };

        // Check that every use of vN in the loop body is `[vN + 1]`
        let body_lines = &lines[loop_start..loop_end];
        let idx_pattern = format!("[{var_name} + 1]");
        let mut all_uses_are_indexed = true;
        let mut has_any_use = false;

        for body_line in body_lines {
            let bl = *body_line;
            // Check if vN appears in this line at all (whole-word)
            let bytes = bl.as_bytes();
            let var_bytes = var_name.as_bytes();
            let mut pos = 0;
            while pos + var_bytes.len() <= bytes.len() {
                if &bytes[pos..pos + var_bytes.len()] == var_bytes {
                    let before_ok = pos == 0 || !is_ident_char(bytes[pos - 1]);
                    let after_ok = pos + var_bytes.len() >= bytes.len()
                        || !is_ident_char(bytes[pos + var_bytes.len()]);
                    if before_ok && after_ok {
                        has_any_use = true;
                        // This occurrence must be part of `[vN + 1]`
                        // Check: byte before should be `[` and bytes after should be ` + 1]`
                        let bracket_before = pos > 0 && bytes[pos - 1] == b'[';
                        let suffix = &bl[pos + var_bytes.len()..];
                        let has_plus_one = suffix.starts_with(" + 1]");
                        if !bracket_before || !has_plus_one {
                            all_uses_are_indexed = false;
                            break;
                        }
                    }
                }
                pos += 1;
            }
            if !all_uses_are_indexed {
                break;
            }
        }

        if !has_any_use || !all_uses_are_indexed {
            i += 1;
            continue;
        }

        // Rewrite the for-loop header
        let new_header = format!("{loop_indent}for {var_name} = 1, {upper_expr} do");
        replacements.insert(i, new_header);

        // Rewrite body lines: replace `[vN + 1]` with `[vN]`
        let replacement_bracket = format!("[{var_name}]");
        for j in loop_start..loop_end {
            if lines[j].contains(&idx_pattern) {
                let new_line = lines[j].replace(&idx_pattern, &replacement_bracket);
                replacements.insert(j, new_line);
            }
        }

        i = loop_end + 1;
    }

    if replacements.is_empty() {
        return;
    }

    let count = replacements
        .keys()
        .filter(|k| lines[**k].trim().starts_with("for "))
        .count();
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
    eprintln!("[molt-luau] Folded range indices in {} loops", count);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_whole_word_preserves_non_ascii_literals() {
        assert_eq!(
            replace_whole_word("local v1: string = \"?\"", "v2", "x"),
            "local v1: string = \"?\""
        );
        assert_eq!(
            replace_whole_word("molt_print(v1, \"?\")", "v1", "value"),
            "molt_print(value, \"?\")"
        );
    }
}
