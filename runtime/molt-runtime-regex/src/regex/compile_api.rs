use super::*;

// ---------------------------------------------------------------------------
// molt_re_compile intrinsic
// ---------------------------------------------------------------------------

/// `molt_re_compile(pattern: str, flags: int) -> int`
///
/// Parse a regex pattern string and return an opaque integer handle.  The
/// compiled `CompiledPattern` is stored in the active runtime registry.
/// Returns -1 and raises `re.error` on parse failure.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_compile(pattern_bits: u64, flags_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(pattern) = string_obj_to_owned(obj_from_bits(pattern_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "pattern must be str");
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "flags must be int");
        };
        match parse_pattern(&pattern, flags) {
            Ok(compiled) => {
                let handle = re_alloc_handle(_py);
                re_store_pattern(_py, handle, compiled);
                MoltObject::from_int(handle).bits()
            }
            Err(msg) => raise_exception::<_>(_py, "ValueError", &msg),
        }
    })
}

// ---------------------------------------------------------------------------
// molt_re_pattern_info intrinsic
// ---------------------------------------------------------------------------

/// `molt_re_pattern_info(handle: int) -> (groups, group_names_dict, flags, warn_pos)`
///
/// Returns a 4-tuple:
///   0: groups      â€” int,   number of capturing groups
///   1: group_names â€” dict,  {name: index}
///   2: flags       â€” int,   effective flags (pattern flags | inline flags)
///   3: warn_pos    â€” int or None,  char position of nested-set warning (or None)
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_pattern_info(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(handle) = to_i64(obj_from_bits(handle_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "handle must be int");
        };
        let guard = regex_state(_py)
            .patterns
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(compiled) = guard.get(&handle) else {
            return raise_exception::<_>(_py, "ValueError", "invalid regex handle");
        };
        // Build group_names dict.
        let mut pairs: Vec<u64> = Vec::with_capacity(compiled.group_names.len() * 2);
        for (name, &idx) in &compiled.group_names {
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let idx_bits = MoltObject::from_int(idx as i64).bits();
            pairs.push(name_bits);
            pairs.push(idx_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();

        let groups_bits = MoltObject::from_int(compiled.group_count as i64).bits();
        let flags_bits_out = MoltObject::from_int(compiled.flags).bits();
        let warn_bits = match compiled.warn_pos {
            Some(pos) => MoltObject::from_int(pos).bits(),
            None => MoltObject::none().bits(),
        };

        let tuple_ptr = alloc_tuple(_py, &[groups_bits, dict_bits, flags_bits_out, warn_bits]);
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}
