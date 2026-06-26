use molt_runtime_core::prelude::*;

use super::common::{
    alloc_list_bits, alloc_str_bits, bits_as_i64, bits_is_none, bits_to_string, cleanup_list,
};

// ─── Tcl parsing intrinsics ──────────────────────────────────────────────────

/// Split a Tcl dict response string into key-value pairs.
///
/// The input `tcl_str` is a whitespace-delimited string of alternating keys
/// and values (the standard Tcl dict format). If `cut_minus` is truthy,
/// leading "-" is stripped from keys.
///
/// Returns a Molt list of [key, value] pairs (each pair is a 2-element list).
/// Raises RuntimeError if the element count is odd.
pub extern "C" fn molt_tk_splitdict(tcl_str_bits: u64, cut_minus_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(tcl_str) = bits_to_string(tcl_str_bits) else {
            return rt_raise_str("TypeError", "splitdict requires a string argument");
        };

        // Determine cut_minus flag
        let cut_minus = if bits_is_none(cut_minus_bits) {
            true // default
        } else if let Some(b) = obj_from_bits(cut_minus_bits).as_bool() {
            b
        } else if let Some(i) = bits_as_i64(cut_minus_bits) {
            i != 0
        } else {
            true
        };

        // Split by whitespace into tokens, handling Tcl brace quoting
        let items = split_tcl_list(&tcl_str);

        if !items.len().is_multiple_of(2) {
            return rt_raise_str(
                "RuntimeError",
                "Tcl list representing a dict is expected to contain an even number of elements",
            );
        }

        // Build list of [key, value] pairs
        let mut pairs: Vec<u64> = Vec::with_capacity(items.len() / 2);
        let mut i = 0;
        while i + 1 < items.len() {
            let mut key = items[i].clone();
            if cut_minus && key.starts_with('-') {
                key = key[1..].to_string();
            }
            let value = &items[i + 1];

            let key_bits = match alloc_str_bits(&key) {
                Ok(bits) => bits,
                Err(bits) => {
                    cleanup_list(&pairs);
                    return bits;
                }
            };
            let value_bits = match alloc_str_bits(value) {
                Ok(bits) => bits,
                Err(bits) => {
                    rt_dec_ref(key_bits);
                    cleanup_list(&pairs);
                    return bits;
                }
            };

            // Build a 2-element list [key, value]
            let pair_elems = [key_bits, value_bits];
            let pair_bits = match alloc_list_bits(&pair_elems) {
                Ok(bits) => bits,
                Err(bits) => {
                    rt_dec_ref(key_bits);
                    rt_dec_ref(value_bits);
                    cleanup_list(&pairs);
                    return bits;
                }
            };
            // alloc_list already inc-ref'd key and value; release our local refs
            rt_dec_ref(key_bits);
            rt_dec_ref(value_bits);

            pairs.push(pair_bits);
            i += 2;
        }

        // Build the outer list of pairs
        match alloc_list_bits(&pairs) {
            Ok(list_bits) => {
                for &pair_bits in &pairs {
                    rt_dec_ref(pair_bits);
                }
                list_bits
            }
            Err(bits) => {
                cleanup_list(&pairs);
                bits
            }
        }
    })
}

/// Parse a Tcl list string, handling brace quoting and backslash escapes.
/// This is a simplified parser sufficient for Tcl dict responses.
fn split_tcl_list(input: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut chars = input.chars().peekable();

    loop {
        // Skip whitespace
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }

        let ch = *chars.peek().unwrap();
        if ch == '{' {
            // Brace-quoted word
            chars.next(); // consume '{'
            let mut depth = 1;
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                chars.next();
                if c == '{' {
                    depth += 1;
                    word.push(c);
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    word.push(c);
                } else {
                    word.push(c);
                }
            }
            items.push(word);
        } else if ch == '"' {
            // Double-quoted word
            chars.next(); // consume '"'
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                chars.next();
                if c == '"' {
                    break;
                } else if c == '\\' {
                    if let Some(&next) = chars.peek() {
                        chars.next();
                        match next {
                            'n' => word.push('\n'),
                            't' => word.push('\t'),
                            '\\' => word.push('\\'),
                            '"' => word.push('"'),
                            other => {
                                word.push('\\');
                                word.push(other);
                            }
                        }
                    } else {
                        word.push('\\');
                    }
                } else {
                    word.push(c);
                }
            }
            items.push(word);
        } else {
            // Bare word — read until whitespace
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                chars.next();
                if c == '\\' {
                    if let Some(&next) = chars.peek() {
                        chars.next();
                        match next {
                            'n' => word.push('\n'),
                            't' => word.push('\t'),
                            '\\' => word.push('\\'),
                            other => {
                                word.push('\\');
                                word.push(other);
                            }
                        }
                    } else {
                        word.push('\\');
                    }
                } else {
                    word.push(c);
                }
            }
            items.push(word);
        }
    }

    items
}
