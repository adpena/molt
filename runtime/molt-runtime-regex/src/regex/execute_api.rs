use super::*;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// molt_re_execute â€” Phase-1b match engine
// ---------------------------------------------------------------------------

/// `molt_re_execute(handle, text, pos, end, mode) -> match_result | None`
///
/// Execute a compiled regex pattern against the given text.
///
/// Arguments:
///   handle â€” integer handle from `molt_re_compile`
///   text   â€” subject string
///   pos    â€” start position (char index)
///   end    â€” end position (char index, exclusive)
///   mode   â€” "match", "search", or "fullmatch"
///
/// Returns:
///   None on no match, or a tuple `(start, end, groups_tuple)` where
///   groups_tuple is a tuple of `(start, end) | None` for each group.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_execute(
    handle_bits: u64,
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    mode_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "handle must be int");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(mode) = string_obj_to_owned(obj_from_bits(mode_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "mode must be str");
        };

        let pos_usize = if pos < 0 { 0usize } else { pos as usize };
        let end_usize = if end < 0 { 0usize } else { end as usize };

        // Look up the compiled pattern.
        let guard = regex_state(_py)
            .patterns
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(compiled) = guard.get(&handle) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };

        // Clone the parts we need so we can drop the lock.
        let root = compiled.root.clone();
        let group_count = compiled.group_count;
        let flags = compiled.flags;
        drop(guard);

        let local_compiled = CompiledPattern {
            root,
            group_count,
            group_names: HashMap::new(), // not needed for matching
            flags,
            warn_pos: None,
        };

        match execute_match(&local_compiled, &text, pos_usize, end_usize, &mode) {
            Some(result) => build_match_result_bits(_py, &result, group_count),
            None => MoltObject::none().bits(),
        }
    })
}

// ---------------------------------------------------------------------------
// molt_re_finditer_collect â€” Phase-1b find-all engine
// ---------------------------------------------------------------------------

/// `molt_re_finditer_collect(handle, text, pos, end) -> list | None`
///
/// Find all non-overlapping matches of a compiled pattern in the text.
///
/// Returns a list of match result tuples `[(start, end, groups), ...]`
/// or None if the pattern handle is invalid.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_finditer_collect(
    handle_bits: u64,
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "handle must be int");
        };
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };

        let pos_usize = if pos < 0 { 0usize } else { pos as usize };
        let end_usize = if end < 0 { 0usize } else { end as usize };

        // Look up the compiled pattern.
        let guard = regex_state(_py)
            .patterns
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(compiled) = guard.get(&handle) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };

        let root = compiled.root.clone();
        let group_count = compiled.group_count;
        let flags = compiled.flags;
        drop(guard);

        let local_compiled = CompiledPattern {
            root,
            group_count,
            group_names: HashMap::new(),
            flags,
            warn_pos: None,
        };

        let chars: Vec<char> = text.chars().collect();
        let text_len = chars.len();
        let end_clamp = end_usize.min(text_len);

        let mut results: Vec<u64> = Vec::new();
        let mut cur = pos_usize;
        let mut prev_empty_match_at: Option<usize> = None;

        while cur <= end_clamp {
            match execute_match(&local_compiled, &text, cur, end_clamp, "search") {
                Some(result) => {
                    let match_start = result.start;
                    let match_end = result.end;

                    // Avoid infinite loop on zero-width matches at the same position.
                    if match_start == match_end {
                        if prev_empty_match_at == Some(match_start) {
                            // Already yielded an empty match here â€” advance.
                            if cur < end_clamp {
                                cur += 1;
                            } else {
                                break;
                            }
                            continue;
                        }
                        prev_empty_match_at = Some(match_start);
                    } else {
                        prev_empty_match_at = None;
                    }

                    let bits = build_match_result_bits(_py, &result, group_count);
                    results.push(bits);

                    if match_end == match_start {
                        // Zero-width match â€” advance by one to avoid infinite loop.
                        if cur < end_clamp {
                            cur = match_start + 1;
                        } else {
                            break;
                        }
                    } else {
                        cur = match_end;
                    }
                }
                None => break,
            }
        }

        let list_ptr = alloc_list(_py, &results);
        if list_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}
