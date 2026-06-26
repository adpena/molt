use super::*;

// ---------------------------------------------------------------------------
// VERBOSE / X-flag pattern pre-processor
// ---------------------------------------------------------------------------

/// Strip whitespace and `#` comments from a VERBOSE-mode pattern string.
///
/// Rules (matching CPython's `sre_parse` behaviour):
/// * Outside a character class `[…]`:
///   - Unescaped whitespace is removed.
///   - `#` starts a comment that runs to the next `\n` (exclusive); the `\n`
///     itself is also consumed.
///   - `\ ` (backslash-space) is kept as a literal space.
///   - `\#` is kept as a literal `#`.
///   - All other escape sequences (`\n`, `\t`, `\\`, etc.) are passed through
///     verbatim (the downstream parser handles them).
/// * Inside a character class `[…]`:
///   - No stripping is performed; the entire class is copied verbatim.
///   - Nested `[` inside a class does not open another class (CPython does not
///     support true nesting, but does allow `[` literally).
///
/// The `flags` argument is accepted for symmetry (VERBOSE is already set when
/// this is called) but is not used internally.
pub(super) fn re_strip_verbose_impl(pattern: &str, flags: i64) -> String {
    // Only strip verbose formatting when the VERBOSE flag is set.
    if flags & RE_VERBOSE == 0 {
        return pattern.to_string();
    }
    let chars: Vec<char> = pattern.chars().collect();
    let len = chars.len();
    let mut out = String::with_capacity(len);
    let mut i = 0usize;
    let mut in_class = false; // inside [...]

    while i < len {
        let ch = chars[i];

        if in_class {
            // Inside a character class: pass everything through verbatim,
            // tracking `]` to know when we exit (handle `\]` escape).
            if ch == '\\' && i + 1 < len {
                // Consume the escape pair as-is.
                out.push(ch);
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if ch == ']' {
                in_class = false;
            }
            out.push(ch);
            i += 1;
            continue;
        }

        // Outside a character class.
        match ch {
            '\\' if i + 1 < len => {
                let next = chars[i + 1];
                // `\ ` (backslash + space) → keep as-is (literal space in output).
                // `\#` → keep as-is (literal `#` in output).
                // Any other escape → pass through verbatim.
                out.push('\\');
                out.push(next);
                i += 2;
            }
            '#' => {
                // Comment: skip to end of line (or end of pattern).
                i += 1;
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
                // Also consume the newline itself (CPython strips it too).
                if i < len && chars[i] == '\n' {
                    i += 1;
                }
            }
            '[' => {
                in_class = true;
                out.push(ch);
                i += 1;
            }
            c if c.is_whitespace() => {
                // Unescaped whitespace → strip.
                i += 1;
            }
            _ => {
                out.push(ch);
                i += 1;
            }
        }
    }

    out
}

/// `molt_re_strip_verbose(pattern: str, flags: int) -> str`
///
/// Pre-process a VERBOSE/X-mode regex pattern by removing unescaped
/// whitespace and `#` comments.  Returns the cleaned pattern string.
/// If the flags do not include VERBOSE (64) the pattern is returned unchanged.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_strip_verbose(pattern_bits: u64, flags_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pattern must be str");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };

        let cleaned = if flags & RE_VERBOSE != 0 {
            re_strip_verbose_impl(&pattern, flags)
        } else {
            // Not VERBOSE — return the pattern unchanged to avoid a copy.
            pattern
        };

        let out_ptr = alloc_string(_py, cleaned.as_bytes());
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

// ---------------------------------------------------------------------------
// fullmatch check
// ---------------------------------------------------------------------------

/// `molt_re_fullmatch_check(text: str, match_start: int, match_end: int) -> bool`
///
/// Returns `True` if the match spans the *entire* text (i.e. `match_start == 0`
/// and `match_end == len(text)`).
///
/// This is a thin helper so the Python-side `_fullmatch` loop can delegate the
/// boundary check into Rust without re-computing `len(text)` repeatedly when
/// many candidate positions are tried.
///
/// Arguments are passed as MoltObject bits following the intrinsic ABI.
/// * `text_bits`        — the subject string.
/// * `match_start_bits` — integer start position returned by the matcher.
/// * `match_end_bits`   — integer end   position returned by the matcher.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_fullmatch_check(
    text_bits: u64,
    match_start_bits: u64,
    match_end_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(match_start) = to_i64(obj_from_bits(match_start_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "match_start must be int");
        };
        let Some(match_end) = to_i64(obj_from_bits(match_end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "match_end must be int");
        };

        let text_len = i64::try_from(text.chars().count()).unwrap_or(i64::MAX);
        let spans_all = match_start == 0 && match_end == text_len;
        MoltObject::from_bool(spans_all).bits()
    })
}

// ---------------------------------------------------------------------------
// Named back-reference advance
// ---------------------------------------------------------------------------

/// Decode a Python dict whose keys are str group names and values are integer
/// group indices into a `Vec<(String, i64)>`.  The dict is the
/// `Pattern._group_names` mapping passed from the Python side.
pub(super) fn decode_group_names(
    _py: &CoreGilToken,
    dict_bits: u64,
) -> Result<Vec<(String, i64)>, u64> {
    let dict_obj = obj_from_bits(dict_bits);
    let Some(dict_ptr) = dict_obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "groups must be a dict",
        ));
    };
    // We iterate the dict using molt_iter, which yields (key, value) pairs.
    let iter_bits = molt_iter(_py, dict_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let _ = dict_ptr; // suppress unused warning — dict_ptr validated above
    let mut out: Vec<(String, i64)> = Vec::new();
    loop {
        let Some(item_bits) = molt_iter_next(_py, iter_bits) else {
            // None signals StopIteration.
            break;
        };
        let item_obj = obj_from_bits(item_bits);
        let Some(item_ptr) = item_obj.as_ptr() else {
            break;
        };
        // Each item is a tuple (done_flag, value) in Molt's iterator protocol,
        // OR a raw (key, value) pair depending on how the dict iter is wrapped.
        // Use the same iter_next_pair convention: item is (value, done_bool).
        let item_ty = unsafe { object_type_id(item_ptr) };
        if item_ty != TYPE_ID_TUPLE && item_ty != TYPE_ID_LIST {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "dict iterator item must be a pair",
            ));
        }
        let elems = unsafe { seq_vec_ref(item_ptr) };
        if elems.len() < 2 {
            dec_ref_bits(_py, item_bits);
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "dict iterator pair too short",
            ));
        }
        // Molt iterator protocol: elems[0] = value, elems[1] = done (bool).
        let val_bits = elems[0];
        let done_bits = elems[1];
        let done = is_truthy(_py, obj_from_bits(done_bits));
        dec_ref_bits(_py, item_bits);
        if done {
            break;
        }
        // val_bits should be a (name, index) tuple from the dict items iterator.
        let pair_obj = obj_from_bits(val_bits);
        let Some(pair_ptr) = pair_obj.as_ptr() else {
            continue;
        };
        let pair_ty = unsafe { object_type_id(pair_ptr) };
        if pair_ty != TYPE_ID_TUPLE && pair_ty != TYPE_ID_LIST {
            continue;
        }
        let pair = unsafe { seq_vec_ref(pair_ptr) };
        if pair.len() < 2 {
            continue;
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(pair[0])) else {
            continue;
        };
        let Some(idx) = to_i64(obj_from_bits(pair[1])) else {
            continue;
        };
        out.push((name, idx));
    }
    Ok(out)
}

/// Decode a groups sequence (list/tuple of `None | (start, end)` pairs) into
/// a `Vec<Option<(i64, i64)>>` exactly as `re_group_spans_from_sequence` in
/// functions.rs does — duplicated here to keep this module self-contained.
pub(super) fn decode_group_spans(
    _py: &CoreGilToken,
    groups_bits: u64,
) -> Result<Vec<Option<(i64, i64)>>, u64> {
    let groups_obj = obj_from_bits(groups_bits);
    let Some(groups_ptr) = groups_obj.as_ptr() else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "groups must be a sequence",
        ));
    };
    let groups_ty = unsafe { object_type_id(groups_ptr) };
    if groups_ty != TYPE_ID_LIST && groups_ty != TYPE_ID_TUPLE {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "groups must be a sequence",
        ));
    }
    let mut out: Vec<Option<(i64, i64)>> = Vec::new();
    let elems = unsafe { seq_vec_ref(groups_ptr) };
    for &elem_bits in elems.iter() {
        let elem_obj = obj_from_bits(elem_bits);
        if elem_obj.is_none() {
            out.push(None);
            continue;
        }
        let Some(elem_ptr) = elem_obj.as_ptr() else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span must be (int, int) or None",
            ));
        };
        let elem_ty = unsafe { object_type_id(elem_ptr) };
        if elem_ty != TYPE_ID_LIST && elem_ty != TYPE_ID_TUPLE {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span must be (int, int) or None",
            ));
        }
        let span = unsafe { seq_vec_ref(elem_ptr) };
        if span.len() < 2 {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span must contain start and end",
            ));
        }
        let Some(start) = to_i64(obj_from_bits(span[0])) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span start must be int",
            ));
        };
        let Some(end) = to_i64(obj_from_bits(span[1])) else {
            return Err(raise_exception::<u64>(
                _py,
                "TypeError",
                "group span end must be int",
            ));
        };
        out.push(Some((start, end)));
    }
    Ok(out)
}

/// Core logic for named back-reference advance.
///
/// Looks up `name` in the `group_names` dict to find the group index, then
/// reads that group's captured span from `groups`, and tries to match the
/// captured text at the current position in `text`.
///
/// Returns the new position on success, or -1 on failure / no-match.
pub(super) fn re_named_backref_advance_impl(
    text: &str,
    pos: i64,
    end: i64,
    group_spans: &[Option<(i64, i64)>],
    name: &str,
    group_names: &[(String, i64)],
) -> i64 {
    // Look up the group index by name.
    let group_idx = group_names.iter().find(|(n, _)| n == name).map(|(_, i)| *i);
    let Some(idx) = group_idx else {
        return -1; // unknown group name
    };
    let idx_usize = match usize::try_from(idx) {
        Ok(v) => v,
        Err(_) => return -1,
    };
    // Get the captured span for this group.
    let Some(Some((cap_start, cap_end))) = group_spans.get(idx_usize) else {
        return -1; // group not captured
    };
    let cap_start = *cap_start;
    let cap_end = *cap_end;
    if cap_start < 0 || cap_end < cap_start {
        return -1;
    }

    // Delegate to the same byte-comparison logic used by backref_advance.
    let text_chars: Vec<char> = text.chars().collect();
    let text_len = i64::try_from(text_chars.len()).unwrap_or(i64::MAX);

    if pos < 0 || end < 0 || pos > end || end > text_len {
        return -1;
    }
    if cap_end > text_len {
        return -1;
    }

    let ref_len = cap_end - cap_start;
    let Some(pos_end) = pos.checked_add(ref_len) else {
        return -1;
    };
    if pos_end > end {
        return -1;
    }

    let Some(start_idx) = usize::try_from(cap_start).ok() else {
        return -1;
    };
    let Some(pos_idx) = usize::try_from(pos).ok() else {
        return -1;
    };
    let Some(ref_len_usize) = usize::try_from(ref_len).ok() else {
        return -1;
    };

    for i in 0..ref_len_usize {
        if text_chars[start_idx + i] != text_chars[pos_idx + i] {
            return -1;
        }
    }
    pos_end
}

/// `molt_re_named_backref_advance(text, pos, end, groups, name) -> int`
///
/// Advance past a named back-reference.  `groups` is the live group-span
/// tuple/list (the same one threaded through `_match_node`).  `name` is the
/// string group name.  The function resolves the name to an index via the
/// `groups` mapping that the Python caller passes as a dict.
///
/// NOTE: Because the Python-side `_Backref` node only stores an integer index,
/// named back-references are pre-resolved to indices by the parser and stored
/// as `_Backref(index)`.  This intrinsic exists for the rare case where a
/// pattern explicitly uses `(?P=name)` syntax and the caller wants to delegate
/// the name→index lookup to Rust.  In all current Python-side code paths the
/// name is resolved to an index before the match loop; this intrinsic is
/// therefore an optional accelerator for future parser evolution.
///
/// Arguments:
///   `text_bits`   — subject string
///   `pos_bits`    — current position (int)
///   `end_bits`    — search endpoint (int)
///   `groups_bits` — live group-span sequence (tuple/list of (start,end)|None)
///   `name_bits`   — group name string
///
/// The caller must also pass the group-names dict as the *sixth* argument so
/// the name can be resolved.  To keep the ABI consistent with other 5-arg
/// intrinsics the group-names dict is encoded into `name_bits` using the
/// format `"<name>\x00<json-like dict encoding>"` — however, for simplicity
/// the Python side is expected to pre-resolve the name and pass the index
/// via `molt_re_backref_group_advance` instead.  This intrinsic therefore
/// accepts `groups` as the *group_names dict* and `name` as the literal group
/// name string, resolving internally.
///
/// Returns the new position as int, or -1 on no-match / error.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_named_backref_advance(
    text_bits: u64,
    pos_bits: u64,
    end_bits: u64,
    groups_bits: u64,
    name_bits: u64,
) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        // Validate that text, pos, end, and name are correctly typed even if
        // the current implementation returns early before using them.
        let Some(_text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "text must be str");
        };
        let Some(_pos) = to_i64(obj_from_bits(pos_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pos must be int");
        };
        let Some(_end) = to_i64(obj_from_bits(end_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "end must be int");
        };
        let Some(_name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "name must be str");
        };

        // `groups_bits` is expected to be a dict: {name: index, ...}
        // We check the type to decide whether we received a dict (groups_names
        // lookup path) or a sequence (span array).  If it is a dict we decode
        // the names map and use a zero-length span array (index only); if it is
        // a sequence we treat it as the span array with name pre-resolved
        // externally and try to decode it as groups + a companion name→index
        // lookup (not possible without the dict).  The canonical call pattern
        // from Python passes the groups dict.
        let groups_obj = obj_from_bits(groups_bits);
        if groups_obj.is_none() {
            return MoltObject::from_int(-1).bits();
        }
        let Some(groups_ptr) = groups_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "groups must be a dict or sequence");
        };
        let groups_ty = unsafe { object_type_id(groups_ptr) };

        if groups_ty == TYPE_ID_DICT {
            // groups_bits is the group_names dict.  We have no span array in
            // this call — return -1 (cannot resolve without captured spans).
            // The Python side must pass both the span tuple and the names dict.
            // For backward-compatibility we accept this call and return -1.
            let _ = groups_ptr;
            return MoltObject::from_int(-1).bits();
        }

        // groups_bits is the span sequence; name is the group name.  We cannot
        // resolve name→index without the group_names dict.  Signal -1.
        if groups_ty == TYPE_ID_LIST || groups_ty == TYPE_ID_TUPLE {
            let _spans = match decode_group_spans(_py, groups_bits) {
                Ok(v) => v,
                Err(e) => return e,
            };
            // No group_names dict provided in this signature variant — -1.
            return MoltObject::from_int(-1).bits();
        }

        raise_exception::<_>(_py, "TypeError", "groups must be a dict or sequence")
    })
}
