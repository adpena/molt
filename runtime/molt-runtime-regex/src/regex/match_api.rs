use super::*;

// ---------------------------------------------------------------------------
// Match object helpers — .group(), .groups(), .groupdict()
// ---------------------------------------------------------------------------
//
// The Rust match engine returns a flat tuple (start, end, groups_tuple).
// These intrinsics operate on that tuple + the original text to provide the
// CPython Match object API efficiently from Rust.

/// `molt_re_match_group(text, match_tuple, *indices) -> str | tuple[str|None, ...]`
///
/// Implements `Match.group(...)`.  `indices` is a tuple of (int | str) group
/// selectors.  If a single index is given, returns a string (or None for
/// unmatched groups).  If multiple indices, returns a tuple.
///
/// `match_tuple` is the `(start, end, groups_tuple)` from `molt_re_execute`.
/// `group_names_bits` is a dict mapping name → index for named groups.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_match_group(
    text_bits: u64,
    match_tuple_bits: u64,
    indices_bits: u64,
    group_names_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let text_chars: Vec<char> = text.chars().collect();

        // Decode match_tuple = (start, end, groups_tuple)
        let Some(mt_ptr) = obj_from_bits(match_tuple_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "match_tuple must be a tuple");
        };
        let mt = unsafe { seq_vec_ref(mt_ptr) };
        if mt.len() < 3 {
            return raise_exception::<_>(_py, "ValueError", "invalid match tuple");
        }
        let Some(m_start) = to_i64(obj_from_bits(mt[0])) else {
            return raise_exception::<_>(_py, "ValueError", "invalid match start");
        };
        let Some(m_end) = to_i64(obj_from_bits(mt[1])) else {
            return raise_exception::<_>(_py, "ValueError", "invalid match end");
        };

        // Decode the groups tuple.
        let Some(groups_ptr) = obj_from_bits(mt[2]).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "groups must be a tuple");
        };
        let group_spans = unsafe { seq_vec_ref(groups_ptr) };

        // Helper: resolve a group index from an int or string selector.
        let resolve_group = |sel_bits: u64| -> Option<usize> {
            if let Some(idx) = to_i64(obj_from_bits(sel_bits)) {
                return Some(idx as usize);
            }
            // Try as string name.
            if let Some(name) = string_obj_to_owned(obj_from_bits(sel_bits))
                && let Some(gn_ptr) = obj_from_bits(group_names_bits).as_ptr()
            {
                let gn_ty = unsafe { object_type_id(gn_ptr) };
                if gn_ty == TYPE_ID_DICT {
                    // Look up name in the dict.
                    if let Some(name_key_bits) = attr_name_bits_from_bytes(_py, name.as_bytes()) {
                        if let Some(val_bits) =
                            unsafe { dict_get_in_place(_py, gn_ptr, name_key_bits) }
                        {
                            dec_ref_bits(_py, name_key_bits);
                            return to_i64(obj_from_bits(val_bits)).map(|v| v as usize);
                        }
                        dec_ref_bits(_py, name_key_bits);
                    }
                }
            }
            None
        };

        // Helper: extract group text for index i (0 = whole match).
        let group_text_bits = |i: usize| -> u64 {
            if i == 0 {
                // Whole match.
                let ms = m_start as usize;
                let me = m_end as usize;
                if ms <= me && me <= text_chars.len() {
                    let s: String = text_chars[ms..me].iter().collect();
                    let ptr = alloc_string(_py, s.as_bytes());
                    if !ptr.is_null() {
                        return MoltObject::from_ptr(ptr).bits();
                    }
                }
                return MoltObject::none().bits();
            }
            // Group i is at index i-1 in the spans tuple (groups are 1-based,
            // but the groups_tuple stores them starting at index 0 for group 1).
            let span_idx = i - 1;
            if span_idx >= group_spans.len() {
                return MoltObject::none().bits();
            }
            let span_bits = group_spans[span_idx];
            if obj_from_bits(span_bits).is_none() {
                return MoltObject::none().bits();
            }
            let Some(span_ptr) = obj_from_bits(span_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            let span = unsafe { seq_vec_ref(span_ptr) };
            if span.len() < 2 {
                return MoltObject::none().bits();
            }
            let Some(gs) = to_i64(obj_from_bits(span[0])) else {
                return MoltObject::none().bits();
            };
            let Some(ge) = to_i64(obj_from_bits(span[1])) else {
                return MoltObject::none().bits();
            };
            if gs < 0 || ge < gs {
                return MoltObject::none().bits();
            }
            let gs = gs as usize;
            let ge = ge as usize;
            if ge > text_chars.len() {
                return MoltObject::none().bits();
            }
            let s: String = text_chars[gs..ge].iter().collect();
            let ptr = alloc_string(_py, s.as_bytes());
            if !ptr.is_null() {
                MoltObject::from_ptr(ptr).bits()
            } else {
                MoltObject::none().bits()
            }
        };

        // Decode indices tuple.
        let Some(indices_ptr) = obj_from_bits(indices_bits).as_ptr() else {
            // No indices → return group(0) = whole match.
            return group_text_bits(0);
        };
        let indices = unsafe { seq_vec_ref(indices_ptr) };

        if indices.is_empty() {
            // group() with no args → group(0)
            return group_text_bits(0);
        }

        if indices.len() == 1 {
            // Single index → return the group directly (not wrapped in tuple).
            let Some(idx) = resolve_group(indices[0]) else {
                return raise_exception::<_>(_py, "IndexError", "no such group");
            };
            return group_text_bits(idx);
        }

        // Multiple indices → return a tuple.
        let mut result: Vec<u64> = Vec::with_capacity(indices.len());
        for &sel_bits in indices.iter() {
            let Some(idx) = resolve_group(sel_bits) else {
                for bits in &result {
                    dec_ref_bits(_py, *bits);
                }
                return raise_exception::<_>(_py, "IndexError", "no such group");
            };
            result.push(group_text_bits(idx));
        }
        let tuple_ptr = alloc_tuple(_py, &result);
        for bits in &result {
            dec_ref_bits(_py, *bits);
        }
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

/// `molt_re_match_groups(text, match_tuple, default) -> tuple[str|None, ...]`
///
/// Implements `Match.groups(default=None)`.  Returns a tuple of all captured
/// groups (1-based).  Unmatched groups use `default`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_match_groups(
    text_bits: u64,
    match_tuple_bits: u64,
    default_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let text_chars: Vec<char> = text.chars().collect();

        let Some(mt_ptr) = obj_from_bits(match_tuple_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "match_tuple must be a tuple");
        };
        let mt = unsafe { seq_vec_ref(mt_ptr) };
        if mt.len() < 3 {
            return raise_exception::<_>(_py, "ValueError", "invalid match tuple");
        }
        let Some(groups_ptr) = obj_from_bits(mt[2]).as_ptr() else {
            let ptr = alloc_tuple(_py, &[]);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        };
        let group_spans = unsafe { seq_vec_ref(groups_ptr) };

        let mut result: Vec<u64> = Vec::with_capacity(group_spans.len());
        for &span_bits in group_spans.iter() {
            if obj_from_bits(span_bits).is_none() {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            }
            let Some(span_ptr) = obj_from_bits(span_bits).as_ptr() else {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            };
            let span = unsafe { seq_vec_ref(span_ptr) };
            if span.len() < 2 {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            }
            let Some(gs) = to_i64(obj_from_bits(span[0])) else {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            };
            let Some(ge) = to_i64(obj_from_bits(span[1])) else {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            };
            if gs < 0 || ge < gs || (ge as usize) > text_chars.len() {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
                continue;
            }
            let s: String = text_chars[gs as usize..ge as usize].iter().collect();
            let ptr = alloc_string(_py, s.as_bytes());
            if !ptr.is_null() {
                result.push(MoltObject::from_ptr(ptr).bits());
            } else {
                inc_ref_bits(_py, default_bits);
                result.push(default_bits);
            }
        }
        let tuple_ptr = alloc_tuple(_py, &result);
        for bits in &result {
            dec_ref_bits(_py, *bits);
        }
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

/// `molt_re_match_groupdict(text, match_tuple, default, group_names) -> dict`
///
/// Implements `Match.groupdict(default=None)`.  Returns a dict mapping named
/// group names to their captured text (or `default` if the group did not
/// participate in the match).
///
/// `group_names` is a dict `{name: index, ...}`.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_match_groupdict(
    text_bits: u64,
    match_tuple_bits: u64,
    default_bits: u64,
    group_names_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let text_chars: Vec<char> = text.chars().collect();

        let Some(mt_ptr) = obj_from_bits(match_tuple_bits).as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "match_tuple must be a tuple");
        };
        let mt = unsafe { seq_vec_ref(mt_ptr) };
        if mt.len() < 3 {
            return raise_exception::<_>(_py, "ValueError", "invalid match tuple");
        }
        let groups_ptr_opt = obj_from_bits(mt[2]).as_ptr();

        // Decode group_names dict.
        let Some(gn_ptr) = obj_from_bits(group_names_bits).as_ptr() else {
            // No group names → empty dict.
            let ptr = alloc_dict_with_pairs(_py, &[]);
            return if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            };
        };
        let gn_ty = unsafe { object_type_id(gn_ptr) };
        if gn_ty != TYPE_ID_DICT {
            return raise_exception::<_>(_py, "TypeError", "group_names must be a dict");
        }

        let result_ptr = alloc_dict_with_pairs(_py, &[]);
        if result_ptr.is_null() {
            return MoltObject::none().bits();
        }

        // Iterate over group_names dict.
        let order = unsafe { dict_order_clone(_py, gn_ptr) };
        for pair in order.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            let name_key_bits = pair[0];
            let idx_bits = pair[1];
            let Some(idx) = to_i64(obj_from_bits(idx_bits)) else {
                continue;
            };

            // Get the group text for this index.
            let val_bits = if let Some(groups_ptr) = groups_ptr_opt {
                let group_spans = unsafe { seq_vec_ref(groups_ptr) };
                let span_idx = (idx as usize).wrapping_sub(1);
                if span_idx < group_spans.len() {
                    let span_bits = group_spans[span_idx];
                    if let Some(span_ptr) = obj_from_bits(span_bits).as_ptr() {
                        let span = unsafe { seq_vec_ref(span_ptr) };
                        if span.len() >= 2 {
                            let gs = to_i64(obj_from_bits(span[0])).unwrap_or(-1);
                            let ge = to_i64(obj_from_bits(span[1])).unwrap_or(-1);
                            if gs >= 0 && ge >= gs && (ge as usize) <= text_chars.len() {
                                let s: String =
                                    text_chars[gs as usize..ge as usize].iter().collect();
                                let ptr = alloc_string(_py, s.as_bytes());
                                if !ptr.is_null() {
                                    MoltObject::from_ptr(ptr).bits()
                                } else {
                                    default_bits
                                }
                            } else {
                                default_bits
                            }
                        } else {
                            default_bits
                        }
                    } else {
                        default_bits
                    }
                } else {
                    default_bits
                }
            } else {
                default_bits
            };

            unsafe {
                dict_set_in_place(_py, result_ptr, name_key_bits, val_bits);
            }
        }

        MoltObject::from_ptr(result_ptr).bits()
    })
}
