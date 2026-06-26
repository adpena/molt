/// Shared lexical and expression predicates for Luau source postprocessing.

pub(super) fn is_simple_literal(s: &str) -> bool {
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

pub(super) fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_ident_char_scalar(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Replace whole-word occurrences of `needle` with `replacement` in `haystack`.
pub(super) fn replace_whole_word(haystack: &str, needle: &str, replacement: &str) -> String {
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

pub(super) fn is_simple_var_ref(s: &str) -> bool {
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

/// Check if `line` contains a whole-word occurrence of `var`.
pub(super) fn contains_whole_word_var(line: &str, var: &str) -> bool {
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

pub(super) fn is_pure_expr(s: &str) -> bool {
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

/// Find the matching closing parenthesis for an opening paren at `open_pos`.
/// Check if an expression contains binary operators at the top level
/// (not inside `[]`, `()`, or `{}`). Used by the sink pass to decide
/// whether inlined expressions need parenthesization.
pub(super) fn has_top_level_binary_op(expr: &str) -> bool {
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
pub(super) fn find_matching_paren(s: &str, open_pos: usize) -> Option<usize> {
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
