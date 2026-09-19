use super::source_text::is_ident_char;
use std::collections::{BTreeMap, BTreeSet};

/// Re-hoist locals that escaped their scope after text-level optimization.
///
/// The copy propagation and other text-level passes can introduce new variable
/// references that cross block boundaries (e.g., propagating `v167` from one
/// while loop into another). This pass detects `local vN = ...` inside blocks
/// (while/for/if) where `vN` is also referenced outside that block, and hoists
/// the declaration to function scope.
pub(super) fn rehoist_escaped_locals(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();

    // Per-function analysis: find function boundaries.
    let mut i = 0;
    let mut insertions: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut removals: BTreeSet<usize> = BTreeSet::new();
    let mut total = 0;

    while i < lines.len() {
        let t = lines[i].trim();
        // Detect function start: `name = function(` or `local function fn_name(`
        let is_func_start = (t.contains("= function(") && t.ends_with(')'))
            || (t.starts_with("local function ") && t.ends_with(')'));
        if !is_func_start {
            i += 1;
            continue;
        }

        let func_start = i;
        // Find function end by counting depth.
        let _func_indent = lines[i].len() - t.len();
        #[allow(unused_assignments)]
        let mut depth = 0i32;
        let mut func_end = i + 1;
        // Count the opening `function` as depth 1
        depth = 1;
        while func_end < lines.len() {
            let ft = lines[func_end].trim();
            let _fi = lines[func_end].len() - ft.len();
            // Count block openers/closers
            if ft == "while true do"
                || ft.starts_with("for ") && ft.ends_with(" do")
                || ft.starts_with("if ") && ft.ends_with(" then")
                || ft.contains("= function(")
                || ft.starts_with("local function ")
                || ft == "do"
                || ft.starts_with("repeat")
            {
                depth += 1;
            }
            if ft == "end" || ft.starts_with("until ") {
                depth -= 1;
                if depth == 0 {
                    // func_end unchanged
                    break;
                }
            }
            func_end += 1;
        }

        // Now analyze this function (func_start..=func_end)
        // Track block depth within the function
        let mut block_depth = 0i32;
        let mut block_id: u32 = 0;
        // Track (depth, block_id, line_idx) for declarations and uses
        let mut var_decl_scope: BTreeMap<String, (i32, u32, usize)> = BTreeMap::new();
        let mut var_uses: BTreeMap<String, Vec<(i32, u32, usize)>> = BTreeMap::new();

        for j in (func_start + 1)..func_end {
            let lt = lines[j].trim();
            // Track block depth and identity
            if lt == "while true do"
                || lt.starts_with("for ") && lt.ends_with(" do")
                || lt.starts_with("if ") && lt.ends_with(" then")
                || lt == "do"
            {
                block_depth += 1;
                block_id += 1;
            } else if lt == "else" || lt.starts_with("elseif ") {
                block_id += 1; // Same depth, new block
            } else if lt == "end" {
                if block_depth > 0 {
                    block_depth -= 1;
                }
                block_id += 1;
            }

            // Track local declarations
            if let Some(rest) = lt.strip_prefix("local v") {
                let var_end = rest
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(rest.len());
                if var_end > 0 && rest[..var_end].chars().all(|c| c.is_ascii_digit()) {
                    let var = format!("v{}", &rest[..var_end]);
                    var_decl_scope
                        .entry(var)
                        .or_insert((block_depth, block_id, j));
                }
            }

            // Track all variable references (vN patterns)
            let bytes = lt.as_bytes();
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
                        if !var.is_empty() {
                            var_uses.entry(var.to_string()).or_default().push((
                                block_depth,
                                block_id,
                                j,
                            ));
                        }
                    }
                } else {
                    pos += 1;
                }
            }
        }

        // Find variables that need rehoisting: declared inside a block but
        // referenced at a shallower depth OR in a different block at same depth.
        let body_indent = if func_start + 1 < func_end {
            let sample = lines[func_start + 1];
            let sample_trimmed = sample.trim();
            &sample[..sample.len() - sample_trimmed.len()]
        } else {
            "\t"
        };

        // Count existing top-scope locals to avoid exceeding Luau's 200 limit.
        let existing_locals: usize = (func_start + 1..func_end)
            .filter(|&li| {
                let lt = lines[li].trim();
                lt.starts_with("local ")
                    && lines[li].starts_with(body_indent)
                    && !lines[li].starts_with(&format!("{body_indent}\t"))
            })
            .count();
        let hoist_budget = 180_usize.saturating_sub(existing_locals);
        let mut hoisted_count = 0usize;

        for (var, (decl_depth, decl_block, decl_line)) in &var_decl_scope {
            if *decl_depth == 0 {
                continue;
            }
            if let Some(uses) = var_uses.get(var) {
                let needs_hoist = uses.iter().any(|(ud, ub, ul)| {
                    (*ud < *decl_depth || (*ud == *decl_depth && *ub != *decl_block))
                        && *ul != *decl_line
                });
                if needs_hoist && hoisted_count < hoist_budget {
                    hoisted_count += 1;
                    // Add a `local vN` at function scope
                    insertions
                        .entry(func_start + 1)
                        .or_default()
                        .push(format!("{body_indent}local {var}"));
                    // Convert the original `local vN = expr` to `vN = expr`
                    let orig_line = lines[*decl_line];
                    let orig_trimmed = orig_line.trim();
                    if let Some(rest) = orig_trimmed.strip_prefix(&format!("local {var}"))
                        && rest.starts_with(" = ")
                    {
                        let line_indent = &orig_line[..orig_line.len() - orig_trimmed.len()];
                        let new_line = format!("{line_indent}{var}{rest}");
                        removals.insert(*decl_line); // Will be replaced
                        insertions.entry(*decl_line).or_default().push(new_line);
                        total += 1;
                    }
                }
            }
        }

        i = func_end + 1;
    }

    if total == 0 {
        return;
    }

    let mut result = String::with_capacity(source.len() + total * 20);
    for (idx, line) in lines.iter().enumerate() {
        if let Some(inserts) = insertions.get(&idx) {
            if removals.contains(&idx) {
                // This line is being replaced — emit the replacement(s)
                for ins in inserts {
                    result.push_str(ins);
                    result.push('\n');
                }
                continue;
            } else {
                // Insert before this line
                for ins in inserts {
                    result.push_str(ins);
                    result.push('\n');
                }
            }
        }
        if !removals.contains(&idx) {
            result.push_str(line);
            result.push('\n');
        }
    }
    *source = result;
    eprintln!("[molt-luau] Re-hoisted {} escaped locals", total);
}

/// Strip dead code after terminators (`break`, `return`, `error(...)`, `continue`).
///
/// In Luau, statements after `break`/`return`/`error()` within the same block
/// are unreachable and the parser rejects them.  This pass removes lines between
/// a terminator and the next `end`/`else`/`elseif`/`until` at the same or lower
/// indent level.
pub(super) fn strip_dead_code_after_terminators(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();
    let mut remove: BTreeSet<usize> = BTreeSet::new();

    for i in 0..lines.len() {
        let t = lines[i].trim();
        let is_terminator = t == "break"
            || t == "continue"
            || t.starts_with("return")
            || (t.starts_with("error(") && t.ends_with(")"));

        if !is_terminator {
            continue;
        }

        let term_indent = lines[i].len() - lines[i].trim_start().len();

        let mut j = i + 1;
        while j < lines.len() {
            let tj = lines[j].trim();
            if tj.is_empty() {
                j += 1;
                continue;
            }
            let j_indent = lines[j].len() - lines[j].trim_start().len();
            if j_indent <= term_indent
                && (tj == "end"
                    || tj == "end)"
                    || tj == "else"
                    || tj.starts_with("elseif ")
                    || tj.starts_with("until "))
            {
                break;
            }
            if j_indent < term_indent {
                break;
            }
            remove.insert(j);
            j += 1;
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
        "[molt-luau] Stripped {} dead-code-after-terminator lines",
        remove.len()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_luau_exception_region_strip_dead_code_after_terminators_removes_duplicate_return() {
        let mut source = [
            "demo = function()",
            "\tlocal v0 = 1",
            "\treturn v0",
            "\treturn _ret_none_18",
            "end",
        ]
        .join("\n");

        strip_dead_code_after_terminators(&mut source);

        assert!(
            !source.contains("_ret_none_18"),
            "same-block return after return must be removed:\n{source}"
        );
    }

    #[test]
    fn test_luau_exception_region_dead_code_strip_preserves_pcall_close() {
        let mut source = [
            "demo = function()",
            "\tlocal __ok_0, __err_0",
            "\t__ok_0, __err_0 = pcall(function()",
            "\t\terror({__type = \"ValueError\", __msg = \"boom\"})",
            "\tend)",
            "\tif not __ok_0 then molt_exception_set_last(__err_0) end",
            "\tlocal caught = molt_exception_last_pending()",
            "\tif caught then",
            "\t\treturn 1",
            "\telse",
            "\t\terror(caught)",
            "\tend",
            "end",
        ]
        .join("\n");

        strip_dead_code_after_terminators(&mut source);

        assert!(
            source.contains("\tend)\n\tif not __ok_0 then molt_exception_set_last(__err_0) end"),
            "pcall close and failure edge must survive error() inside protected body:\n{source}"
        );
        assert!(
            source.contains("\tif caught then\n\t\treturn 1\n\telse"),
            "handler match branch must remain balanced after pcall close:\n{source}"
        );
    }
}
