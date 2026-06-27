use super::source_text::is_ident_char;
use std::collections::{BTreeMap, BTreeSet};

/// Eliminate `local vN = nil -- [missing]` / `local vM = {vN}` pairs.
///
/// These arise from Python's default-argument mechanism: the IR creates
/// a `missing` sentinel wrapped in a single-element callargs table.
/// When the nil variable is only used in the wrapper, we can replace the
/// wrapper with `{nil}` and remove the nil declaration entirely.
pub(super) fn eliminate_nil_missing_wrappers(source: &mut String) {
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
pub(super) fn strip_unbound_local_checks(source: &mut String) {
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
pub(super) fn strip_dead_locals_dict_stores(source: &mut String) {
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
