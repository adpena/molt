use super::source_text::is_ident_char;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn strip_trailing_continue(source: &mut String) {
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
pub(super) fn simplify_comparison_break(source: &mut String) {
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

/// Index folding for range loops: when `for vN = 0, expr - 1 do` and every use
/// of `vN` in the loop body is `[vN + 1]`, rewrite to `for vN = 1, expr do`
/// and replace `[vN + 1]` with `[vN]`, eliminating one ADD per iteration.
pub(super) fn fold_range_indices(source: &mut String) {
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
