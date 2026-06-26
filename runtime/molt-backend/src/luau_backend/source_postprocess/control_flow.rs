use super::is_ident_char;
use std::collections::{BTreeMap, BTreeSet};

/// Strip dead gotos (targets don't exist) and orphaned labels (no goto points to them).
/// Also strips the containing if-block when the goto is the only statement inside.
/// Strip exception-frame cleanup blocks: the pattern
///   local vN = nil -- [exception_last]
///   local vM = nil; vM = nil; local vP = vN == vM; local vQ = not vP;
///   if vQ then error(vN); goto label_X; end; ::label_X::
/// These are dead code in Luau and the goto-past-local causes syntax errors.
pub(super) fn strip_exception_cleanup_blocks(source: &mut String) {
    let mut lines: Vec<String> = source.lines().map(|l| l.to_string()).collect();
    let mut changed = true;
    let mut total_removed = 0;

    // Iterate until no more patterns found (handles consecutive blocks).
    while changed {
        changed = false;
        let mut remove: BTreeSet<usize> = BTreeSet::new();

        for i in 0..lines.len() {
            let t = lines[i].trim().to_string();
            // Match: `if vN then` or `if not vN then` where next line(s) contain error+goto
            if (t.starts_with("if ") || t.starts_with("if not ")) && t.ends_with(" then") {
                let mut j = i + 1;
                let mut has_error = false;
                let mut has_goto = false;
                let mut goto_label = String::new();
                while j < lines.len() {
                    let tj = lines[j].trim().to_string();
                    if tj.starts_with("error(") {
                        has_error = true;
                    } else if let Some(target) = tj.strip_prefix("goto ") {
                        has_goto = true;
                        goto_label = target.to_string();
                    } else if tj == "end" {
                        if has_error && has_goto {
                            // Only strip the proven-dead cleanup shape rooted
                            // in `exception_last = nil`.  Real pcall-backed
                            // handler dispatch can also contain error+goto in
                            // an unmatched branch and must remain executable.
                            let mut setup_remove: Vec<usize> = Vec::new();
                            let mut saw_dead_exception_last = false;
                            let mut k = i;
                            while k > 0 {
                                k -= 1;
                                let tk = lines[k].trim();
                                if tk.starts_with("local ")
                                    && (tk.contains(" == ") || tk.contains("not "))
                                {
                                    setup_remove.push(k);
                                } else if (tk.ends_with("= nil")
                                    || tk.ends_with("= nil -- [exception_last]"))
                                    && !tk.contains("--[")
                                {
                                    // Only remove lines where the ENTIRE RHS is nil
                                    // (exception cleanup vars), not lines that happen
                                    // to contain "= nil" as part of a larger expression.
                                    if tk.contains("-- [exception_last]") {
                                        saw_dead_exception_last = true;
                                    }
                                    setup_remove.push(k);
                                } else if tk.contains("-- [exception_last]") {
                                    saw_dead_exception_last = true;
                                    setup_remove.push(k);
                                } else {
                                    break;
                                }
                            }
                            if saw_dead_exception_last {
                                // Found the pattern. Remove from i to j inclusive.
                                for k in i..=j {
                                    remove.insert(k);
                                }
                                // Also remove the matching label if it follows.
                                if j + 1 < lines.len() {
                                    let label_line = lines[j + 1].trim().to_string();
                                    if label_line == format!("::{goto_label}::") {
                                        remove.insert(j + 1);
                                    }
                                }
                                for k in setup_remove {
                                    remove.insert(k);
                                }
                            }
                        }
                        break;
                    } else if !tj.is_empty() && !tj.starts_with("--") {
                        break; // Not our pattern
                    }
                    j += 1;
                }
            }
        }

        if !remove.is_empty() {
            changed = true;
            total_removed += remove.len();
            let mut new_lines = Vec::with_capacity(lines.len());
            for (i, line) in lines.iter().enumerate() {
                if !remove.contains(&i) {
                    new_lines.push(line.clone());
                }
            }
            lines = new_lines;
        }
    }

    if total_removed > 0 {
        let mut result = String::with_capacity(source.len());
        for line in &lines {
            result.push_str(line);
            result.push('\n');
        }
        *source = result;
        eprintln!(
            "[molt-luau] Stripped {} exception-cleanup lines",
            total_removed
        );
    }
}

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
        // Detect function start: `XXX = function(` or `local function XXX(`
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

pub(super) fn strip_dead_gotos_and_labels(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();

    // Collect all labels and goto targets.
    let mut existing_labels: BTreeSet<String> = BTreeSet::new();
    let mut goto_targets: BTreeSet<String> = BTreeSet::new();
    for line in &lines {
        let t = line.trim();
        if t.starts_with("::") && t.ends_with("::") && t.len() > 4 {
            existing_labels.insert(t[2..t.len() - 2].to_string());
        }
        if let Some(target) = t.strip_prefix("goto ") {
            goto_targets.insert(target.to_string());
        }
        // Inline gotos: `if ... then goto label_N end`
        if let Some(pos) = t.find("then goto ") {
            let after = &t[pos + 10..];
            if let Some(end_pos) = after.find(' ') {
                goto_targets.insert(after[..end_pos].to_string());
            }
        }
    }

    let mut remove: BTreeSet<usize> = BTreeSet::new();

    // Remove gotos whose target label doesn't exist.
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if let Some(target) = t.strip_prefix("goto ")
            && !existing_labels.contains(target)
        {
            remove.insert(i);
            // Also remove surrounding if/end block if the goto was the only statement.
            if i > 0 && i + 1 < lines.len() {
                let prev = lines[i - 1].trim();
                let next = lines[i + 1].trim();
                if prev.ends_with(" then") && next == "end" {
                    // Check there's an error() before the goto too
                    if i >= 2 && lines[i - 2].trim().starts_with("error(") {
                        remove.insert(i - 2); // error()
                    }
                    remove.insert(i - 1); // if ... then
                    remove.insert(i + 1); // end
                    // Also remove the comparison setup before the if
                    if i >= 3 {
                        let comp = lines[i - 3].trim();
                        if comp.starts_with("if not ") && comp.ends_with(" then") {
                            // This is the outer pattern, already captured
                        }
                    }
                }
            }
        }
    }

    // Remove goto-to-immediately-next-label (dead jump pattern).
    // Pattern: `goto label_N` followed by `::label_N::` on the next non-empty line.
    for i in 0..lines.len().saturating_sub(1) {
        let t = lines[i].trim();
        if let Some(target) = t.strip_prefix("goto ") {
            // Find next non-empty line
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }
            if j < lines.len() {
                let next = lines[j].trim();
                if next == format!("::{target}::") {
                    remove.insert(i);
                }
            }
        }
    }

    // Rebuild goto_targets excluding removed gotos, then remove orphaned labels.
    let mut live_goto_targets: BTreeSet<String> = BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        if remove.contains(&i) {
            continue;
        }
        let t = line.trim();
        if let Some(target) = t.strip_prefix("goto ") {
            live_goto_targets.insert(target.to_string());
        }
        if let Some(pos) = t.find("then goto ") {
            let after = &t[pos + 10..];
            if let Some(end_pos) = after.find(' ') {
                live_goto_targets.insert(after[..end_pos].to_string());
            }
        }
    }
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("::") && t.ends_with("::") && t.len() > 4 {
            let label = &t[2..t.len() - 2];
            if !live_goto_targets.contains(label) {
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
    eprintln!("[molt-luau] Stripped {} dead gotos/labels", remove.len());
}

fn label_name_from_line(line: &str) -> Option<String> {
    let t = line.trim();
    if t.starts_with("::") && t.ends_with("::") && t.len() > 4 {
        Some(t[2..t.len() - 2].to_string())
    } else {
        None
    }
}

fn parse_pcall_failure_goto(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if !t.starts_with("if not __ok_") || !t.ends_with(" end") {
        return None;
    }
    let (condition, after_condition) = t.split_once(" then goto ")?;
    let target = after_condition.strip_suffix(" end")?.trim();
    if target.is_empty() {
        return None;
    }
    Some((condition.to_string(), target.to_string()))
}

fn parse_inline_conditional_goto(line: &str) -> Option<(String, String)> {
    let t = line.trim();
    if !t.starts_with("if ") || !t.ends_with(" end") {
        return None;
    }
    let (condition, after_condition) = t.split_once(" then goto ")?;
    let target = after_condition.strip_suffix(" end")?.trim();
    if target.is_empty() || target.contains(' ') {
        return None;
    }
    Some((condition.to_string(), target.to_string()))
}

fn standalone_goto_target(line: &str) -> Option<String> {
    line.trim()
        .strip_prefix("goto ")
        .map(str::trim)
        .and_then(|target| {
            (!target.is_empty() && !target.contains(' ')).then(|| target.to_string())
        })
}

fn push_reindented_line(
    out: &mut Vec<String>,
    line: &str,
    base_indent: &str,
    child_indent: &str,
    drop_labels: bool,
) {
    if drop_labels && label_name_from_line(line).is_some() {
        return;
    }
    if line.trim().is_empty() {
        out.push(String::new());
        return;
    }
    let rest = line.strip_prefix(base_indent).unwrap_or(line);
    out.push(format!("{child_indent}{rest}"));
}

/// Convert canonical pcall failure-edge regions to structured Luau.
///
/// TIR lays out exception regions as:
///
/// ```text
/// if not __ok_N then goto handler end
/// success-prefix
/// ::shared_continuation::
/// common-continuation
/// ::handler::
/// handler-body
/// goto shared_continuation
/// ```
///
/// The final handler jump is often backward because the common continuation is
/// already laid out in the success path. Generic goto elimination cannot express
/// that without duplicating control flow, so normalize it first:
///
/// ```text
/// if not __ok_N then
///   handler-body
/// else
///   success-prefix
/// end
/// common-continuation
/// ```
pub(super) fn structure_pcall_failure_blocks(source: &mut String) {
    let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
    let mut changed = false;

    'restart: loop {
        let mut i = 0usize;
        while i < lines.len() {
            let Some((condition, handler_label)) = parse_pcall_failure_goto(&lines[i]) else {
                i += 1;
                continue;
            };

            let Some(handler_line) = ((i + 1)..lines.len()).find(|&idx| {
                label_name_from_line(&lines[idx]).as_deref() == Some(handler_label.as_str())
            }) else {
                i += 1;
                continue;
            };

            let mut handler_jump_line = None;
            let mut continuation_label = None;
            let base_indent_len = lines[i].len() - lines[i].trim_start().len();
            let base_indent = lines[i][..base_indent_len].to_string();
            for (idx, line) in lines.iter().enumerate().skip(handler_line + 1) {
                if line.len() - line.trim_start().len() != base_indent_len {
                    continue;
                }
                if let Some(target) = standalone_goto_target(line) {
                    let target_line = ((i + 1)..handler_line).find(|&candidate| {
                        label_name_from_line(&lines[candidate]).as_deref() == Some(target.as_str())
                    });
                    if target_line.is_some() {
                        handler_jump_line = Some(idx);
                        continuation_label = Some((target, target_line.unwrap()));
                        break;
                    }
                }
                if idx > handler_line && parse_pcall_failure_goto(line).is_some() {
                    break;
                }
            }

            let Some(handler_jump_line) = handler_jump_line else {
                i += 1;
                continue;
            };
            let Some((_continuation_label, continuation_line)) = continuation_label else {
                i += 1;
                continue;
            };

            let child_indent = format!("{base_indent}\t");
            let success_prefix = &lines[(i + 1)..continuation_line];
            let common = &lines[(continuation_line + 1)..handler_line];
            let handler = &lines[(handler_line + 1)..handler_jump_line];

            let mut replacement: Vec<String> = Vec::new();
            replacement.push(format!("{base_indent}{condition} then"));
            for line in handler {
                push_reindented_line(&mut replacement, line, &base_indent, &child_indent, true);
            }
            if success_prefix
                .iter()
                .any(|line| !line.trim().is_empty() && label_name_from_line(line).is_none())
            {
                replacement.push(format!("{base_indent}else"));
                for line in success_prefix {
                    push_reindented_line(&mut replacement, line, &base_indent, &child_indent, true);
                }
            }
            replacement.push(format!("{base_indent}end"));
            replacement.extend(common.iter().cloned());

            lines.splice(i..=handler_jump_line, replacement);
            changed = true;
            continue 'restart;
        }
        break;
    }

    if changed {
        *source = lines.join("\n");
        source.push('\n');
    }
}

/// Convert canonical forward-goto diamonds to structured Luau if/else blocks.
///
/// TIR's generic CFG form often lowers an if/else as:
///
/// ```text
/// if cond then goto true_label end
/// false-arm
/// goto join_label
/// ::true_label::
/// true-arm
/// ::join_label::
/// join
/// ```
///
/// Generic skip-flag goto elimination can express this but leaves fragile nested
/// empty blocks after earlier dead-goto cleanup.  This pass consumes the whole
/// diamond as one fact and emits the native Luau control structure.
pub(super) fn structure_forward_if_else_blocks(source: &mut String) {
    let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
    let mut changed = false;

    'restart: loop {
        let mut i = 0usize;
        while i < lines.len() {
            let Some((condition, true_label)) = parse_inline_conditional_goto(&lines[i]) else {
                i += 1;
                continue;
            };

            let Some(true_label_line) = ((i + 1)..lines.len()).find(|&idx| {
                label_name_from_line(&lines[idx]).as_deref() == Some(true_label.as_str())
            }) else {
                i += 1;
                continue;
            };

            let base_indent_len = lines[i].len() - lines[i].trim_start().len();
            let base_indent = lines[i][..base_indent_len].to_string();
            let mut false_goto_line = None;
            let mut join_label = None;
            let mut join_label_line = None;

            for idx in (i + 1)..true_label_line {
                if lines[idx].len() - lines[idx].trim_start().len() != base_indent_len {
                    continue;
                }
                let Some(target) = standalone_goto_target(&lines[idx]) else {
                    continue;
                };
                let Some(target_line) = ((true_label_line + 1)..lines.len()).find(|&candidate| {
                    label_name_from_line(&lines[candidate]).as_deref() == Some(target.as_str())
                }) else {
                    continue;
                };
                false_goto_line = Some(idx);
                join_label = Some(target);
                join_label_line = Some(target_line);
                break;
            }

            let (Some(false_goto_line), Some(join_label), Some(join_label_line)) =
                (false_goto_line, join_label, join_label_line)
            else {
                i += 1;
                continue;
            };

            let false_arm = &lines[(i + 1)..false_goto_line];
            let true_arm_start = true_label_line + 1;
            let mut true_arm_end = join_label_line;
            while true_arm_end > true_arm_start {
                let prev = true_arm_end - 1;
                if lines[prev].trim().is_empty() {
                    true_arm_end = prev;
                    continue;
                }
                if standalone_goto_target(&lines[prev]).as_deref() == Some(join_label.as_str()) {
                    true_arm_end = prev;
                }
                break;
            }
            let true_arm = &lines[true_arm_start..true_arm_end];

            if false_arm
                .iter()
                .any(|line| label_name_from_line(line).is_some())
                || true_arm
                    .iter()
                    .any(|line| label_name_from_line(line).is_some())
            {
                i += 1;
                continue;
            }

            let child_indent = format!("{base_indent}\t");
            let mut replacement: Vec<String> = Vec::new();
            replacement.push(format!("{base_indent}{condition} then"));
            for line in true_arm {
                push_reindented_line(&mut replacement, line, &base_indent, &child_indent, false);
            }
            if false_arm.iter().any(|line| !line.trim().is_empty()) {
                replacement.push(format!("{base_indent}else"));
                for line in false_arm {
                    push_reindented_line(
                        &mut replacement,
                        line,
                        &base_indent,
                        &child_indent,
                        false,
                    );
                }
            }
            replacement.push(format!("{base_indent}end"));

            lines.splice(i..=join_label_line, replacement);
            changed = true;
            continue 'restart;
        }
        break;
    }

    if changed {
        *source = lines.join("\n");
        source.push('\n');
    }
}

/// Eliminate goto/label pairs from emitted Luau.
///
/// Standard Luau does NOT support `goto` or `::label::` syntax (unlike Lua 5.2+).
/// This pass converts all goto/label patterns to structured control flow:
///
/// 1. **Dead gotos** (immediately after `error(...)`) are removed — `error()` throws
///    so the goto is unreachable.
/// 2. **Trivial forward gotos** where the target label is the next non-label,
///    non-empty line are removed (jump to immediately next statement).
/// 3. **Remaining forward gotos** are replaced with a skip-flag pattern:
///    ```text
///    local _molt_skip_N = false
///    ...
///    _molt_skip_N = true  -- replaces: goto label_N
///    if not _molt_skip_N then
///      ... code between goto and label ...
///    end
///    -- (label removed)
///    ```
/// 4. All `::label_N::` lines are removed.
pub(super) fn eliminate_goto_labels(source: &mut String) {
    let lines: Vec<String> = source.lines().map(|l| l.to_string()).collect();
    if lines.is_empty() {
        return;
    }

    // Phase 1: Collect label positions and goto positions.
    let mut label_positions: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut goto_positions: Vec<(usize, String)> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("::") && t.ends_with("::") && t.len() > 4 {
            let label = t[2..t.len() - 2].to_string();
            label_positions.entry(label).or_default().push(i);
        }
        // Standalone goto
        if t.starts_with("goto ") && !t.contains("then goto") {
            let target = t[5..].trim().to_string();
            goto_positions.push((i, target));
        }
        // Inline goto in if block: `if ... then goto label_N end`
        if let Some(pos) = t.find("then goto ") {
            let after = &t[pos + 10..];
            if let Some(end_pos) = after.find(' ') {
                let target = after[..end_pos].to_string();
                goto_positions.push((i, target));
            }
        }
    }

    if goto_positions.is_empty() && label_positions.is_empty() {
        return;
    }

    // Phase 2: Classify gotos.
    let mut remove: BTreeSet<usize> = BTreeSet::new();
    let mut live_gotos: Vec<(usize, String, usize)> = Vec::new();

    for (goto_line, label_name) in &goto_positions {
        let goto_line = *goto_line;
        let t = lines[goto_line].trim().to_string();

        let is_inline = t.contains("then goto ") && t.ends_with(" end");

        // Check if previous non-empty line is error() — dead code after throw.
        let mut dead = false;
        if !is_inline {
            let mut prev = if goto_line > 0 { goto_line - 1 } else { 0 };
            while prev > 0 && lines[prev].trim().is_empty() {
                prev -= 1;
            }
            if lines[prev].trim().starts_with("error(") {
                dead = true;
            }
        }

        if dead {
            remove.insert(goto_line);
            continue;
        }

        // Find the nearest forward label with this name (same name can
        // appear in multiple functions; pick the closest one after the goto).
        let nearest_forward = label_positions
            .get(label_name)
            .and_then(|positions| positions.iter().copied().filter(|&p| p > goto_line).min());
        let any_backward = label_positions
            .get(label_name)
            .map(|positions| positions.iter().any(|&p| p <= goto_line))
            .unwrap_or(false);

        if let Some(label_line) = nearest_forward {
            // Check if goto jumps to immediately next non-label, non-empty line.
            if !is_inline {
                let mut next = goto_line + 1;
                while next < lines.len() {
                    let nt = lines[next].trim();
                    if nt.is_empty() || (nt.starts_with("::") && nt.ends_with("::")) {
                        next += 1;
                    } else {
                        break;
                    }
                }
                if next >= label_line {
                    remove.insert(goto_line);
                    continue;
                }
            }

            live_gotos.push((goto_line, label_name.clone(), label_line));
        } else if any_backward {
            // Backward goto — remove as dead.
            remove.insert(goto_line);
        } else {
            // No matching label at all — orphan goto, remove.
            remove.insert(goto_line);
        }
    }

    // Phase 3: Remove ALL label lines (all positions for all label names).
    for positions in label_positions.values() {
        for &line_idx in positions {
            remove.insert(line_idx);
        }
    }

    // Phase 4: For live gotos, generate flag-based structured control flow.
    // Use composite key "label_name@target_line" to distinguish same-named
    // labels in different functions.
    let mut gotos_by_target: BTreeMap<(String, usize), Vec<usize>> = BTreeMap::new();
    for (goto_line, label_name, label_line) in &live_gotos {
        gotos_by_target
            .entry((label_name.clone(), *label_line))
            .or_default()
            .push(*goto_line);
    }

    let mut insert_before: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    let mut replacements: BTreeMap<usize, String> = BTreeMap::new();
    let mut insert_after: BTreeMap<usize, Vec<String>> = BTreeMap::new();

    for ((label_name, label_line), goto_lines) in &gotos_by_target {
        let label_line = *label_line;
        {
            let flag_name = format!(
                "_molt_skip_{}_{}",
                label_name.replace("label_", ""),
                label_line
            );

            let first_goto = goto_lines[0];
            let indent = lines[first_goto].len() - lines[first_goto].trim_start().len();
            let indent_str: String = lines[first_goto][..indent].to_string();

            insert_before
                .entry(first_goto)
                .or_default()
                .push(format!("{indent_str}local {flag_name} = false"));

            for &goto_line in goto_lines {
                let t = lines[goto_line].trim();
                let goto_indent = lines[goto_line].len() - lines[goto_line].trim_start().len();
                let goto_indent_str: String = lines[goto_line][..goto_indent].to_string();

                if t.starts_with("goto ") {
                    replacements.insert(goto_line, format!("{goto_indent_str}{flag_name} = true"));
                    remove.remove(&goto_line);
                } else if t.contains("then goto ") && t.ends_with(" end") {
                    let replaced = t.replace(
                        &format!("goto {label_name}"),
                        &format!("{flag_name} = true"),
                    );
                    replacements.insert(goto_line, format!("{goto_indent_str}{replaced}"));
                    remove.remove(&goto_line);
                }
            }

            let last_goto = *goto_lines.iter().max().unwrap();
            let wrap_start = last_goto + 1;
            let wrap_end = label_line;

            let has_code = (wrap_start..wrap_end).any(|i| {
                let t = lines[i].trim();
                !(t.is_empty() || (t.starts_with("::") && t.ends_with("::")))
            });

            if has_code {
                insert_after
                    .entry(last_goto)
                    .or_default()
                    .push(format!("{indent_str}if not {flag_name} then"));
                insert_before
                    .entry(wrap_end)
                    .or_default()
                    .push(format!("{indent_str}end"));
            }
        }
    }

    // Phase 5: Rebuild the source.
    let mut result = String::with_capacity(source.len());
    for (i, line) in lines.iter().enumerate() {
        if let Some(inserts) = insert_before.get(&i) {
            for ins in inserts {
                result.push_str(ins);
                result.push('\n');
            }
        }

        if remove.contains(&i) {
            // Skip removed line.
        } else if let Some(replacement) = replacements.get(&i) {
            result.push_str(replacement);
            result.push('\n');
        } else {
            result.push_str(line);
            result.push('\n');
        }

        if let Some(inserts) = insert_after.get(&i) {
            for ins in inserts {
                result.push_str(ins);
                result.push('\n');
            }
        }
    }

    *source = result;
    let total_gotos = goto_positions.len();
    let live_count = live_gotos.len();
    let dead_count = total_gotos - live_count;
    eprintln!(
        "[molt-luau] Eliminated goto/labels: {} dead, {} converted to structured flow, {} labels removed",
        dead_count,
        live_count,
        label_positions.values().map(|v| v.len()).sum::<usize>()
    );
}

/// Strip orphaned label definitions that have no corresponding goto.
/// The `eliminate_goto_labels` pass removes gotos but may leave the
/// `::label_N::` definitions behind, which Luau's parser rejects.
#[allow(dead_code)]
pub(super) fn strip_orphaned_labels(source: &mut String) {
    let lines: Vec<&str> = source.lines().collect();
    // Collect all goto targets
    let mut goto_targets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.starts_with("goto ") {
            let target = trimmed.strip_prefix("goto ").unwrap().trim().to_string();
            goto_targets.insert(target);
        }
    }
    // Remove label definitions that have no goto
    let mut remove: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("::") && trimmed.ends_with("::") {
            let label_name = &trimmed[2..trimmed.len() - 2];
            if !goto_targets.contains(label_name) {
                remove.insert(i);
            }
        }
    }
    if !remove.is_empty() {
        let count = remove.len();
        let new_lines: Vec<&str> = lines
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !remove.contains(i))
            .map(|(_, l)| l)
            .collect();
        *source = new_lines.join("\n");
        eprintln!("[molt-luau] Stripped {count} orphaned label definitions");
    }
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
    fn test_luau_exception_region_pcall_failure_blocks_structure_backward_continuation() {
        let mut source = [
            "demo = function()",
            "\tif not __ok_0 then goto label_5 end",
            "\t::label_7::",
            "\t_bb8_arg0 = module_obj",
            "\t::label_9::",
            "\tmolt_print(\"after-exc\")",
            "\t::label_5::",
            "\tlocal _v23 = _bb1_arg0",
            "\t_bb8_arg0 = _v23",
            "\tgoto label_9",
            "end",
        ]
        .join("\n");

        structure_pcall_failure_blocks(&mut source);

        assert!(
            source.contains(
                "if not __ok_0 then\n\t\tlocal _v23 = _bb1_arg0\n\t\t_bb8_arg0 = _v23\n\telse\n\t\t_bb8_arg0 = module_obj\n\tend\n\tmolt_print(\"after-exc\")"
            ),
            "pcall failure handler must be structured before the shared continuation:\n{source}"
        );
        assert!(
            !source.contains("goto label_9") && !source.contains("::label_5::"),
            "structured pcall block must consume the handler goto/label:\n{source}"
        );
    }

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
            "\tif not __ok_0 then goto label_2 end",
            "\t::label_2::",
            "\tlocal caught = __err_0",
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
            source.contains("\tend)\n\tif not __ok_0 then goto label_2 end"),
            "pcall close and failure edge must survive error() inside protected body:\n{source}"
        );
        assert!(
            source.contains("\tif caught then\n\t\treturn 1\n\telse"),
            "handler match branch must remain balanced after pcall close:\n{source}"
        );
    }
}
