use super::*;
use molt_runtime_core::BuiltinExc;

// ---------------------------------------------------------------------------
// molt_re_compile intrinsic
// ---------------------------------------------------------------------------

/// `molt_re_compile(pattern: str | bytes, flags: int) -> PatternHandle`
///
/// Parse a regex pattern string and return an owned immutable native handle.
/// Returns -1 and raises `re.error` on parse failure.
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_compile(pattern_bits: u64, flags_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let pattern_obj = obj_from_bits(pattern_bits);
        let is_bytes = pattern_obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) } == TYPE_ID_BYTES);
        let pattern = if is_bytes {
            let Some(bytes) = (unsafe { bytes_obj_to_borrowed(pattern_obj) }) else {
                if rt_exception_pending() {
                    return MoltObject::none().bits();
                }
                return raise_exception::<_>(
                    _py,
                    BuiltinExc::TypeError,
                    "first argument must be string or compiled pattern",
                );
            };
            let mut units = Vec::new();
            if let Err(error) = units.try_reserve_exact(bytes.len()) {
                return match_error_result(_py, error.into());
            }
            units.extend(bytes.iter().copied().map(u32::from));
            units
        } else {
            match string_codepoints(
                _py,
                pattern_bits,
                "first argument must be string or compiled pattern",
            ) {
                Ok(units) => units,
                Err(bits) => return bits,
            }
        };
        let Some(flags) = to_i64(obj_from_bits(flags_bits)) else {
            return raise_exception::<_>(_py, BuiltinExc::TypeError, "flags must be int");
        };
        if is_bytes && flags & RE_UNICODE != 0 {
            return raise_exception::<_>(
                _py,
                BuiltinExc::ValueError,
                "cannot use UNICODE flag with a bytes pattern",
            );
        }
        if !is_bytes && flags & RE_LOCALE != 0 {
            return raise_exception::<_>(
                _py,
                BuiltinExc::ValueError,
                "cannot use LOCALE flag with a str pattern",
            );
        }
        match parse_pattern_typed(pattern, flags, is_bytes) {
            Ok(parsed) => {
                let compiled = match parsed.compile() {
                    Ok(compiled) => compiled,
                    Err(error) => return match_error_result(_py, error),
                };
                match pattern_handle_new(std::sync::Arc::new(PatternHandle::new(compiled))) {
                    Some(bits) => bits,
                    None => MoltObject::none().bits(),
                }
            }
            Err(msg) => super::functions_re::syntax_error_result(_py, &msg),
        }
    })
}

// ---------------------------------------------------------------------------
// molt_re_pattern_info intrinsic
// ---------------------------------------------------------------------------

/// `molt_re_pattern_info(handle: PatternHandle) -> (groups, group_names_dict, flags, warn_pos)`
///
/// Returns a 4-tuple:
///   0: groups      â€” int,   number of capturing groups
///   1: group_names â€” dict,  {name: index}
///   2: flags       â€” int,   effective flags (pattern flags | inline flags)
///   3: warn_pos    â€” int or None,  char position of nested-set warning (or None)
#[unsafe(no_mangle)]
pub extern "C" fn molt_re_pattern_info(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(compiled) = pattern_handle_arc(handle_bits) else {
            return raise_exception::<_>(_py, BuiltinExc::ValueError, "invalid regex handle");
        };
        // Project the canonical group-open order directly. Match/groupdict
        // consumers retain this declaration order without sorting per match.
        let mut pairs = Vec::with_capacity(compiled.compiled.group_names.len() * 2);
        for (name, &idx) in &compiled.compiled.group_names {
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let idx_bits = MoltObject::from_int(idx as i64).bits();
            pairs.push(unsafe { OwnedRuntimeValue::from_owned_bits(_py, name_bits) });
            pairs.push(unsafe { OwnedRuntimeValue::from_owned_bits(_py, idx_bits) });
        }
        let dict_ptr = alloc_dict_with_pairs(_py, OwnedRuntimeValue::as_bits_slice(&pairs));
        if dict_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let dictionary = unsafe {
            OwnedRuntimeValue::from_owned_bits(_py, MoltObject::from_ptr(dict_ptr).bits())
        };
        drop(pairs);

        let groups_bits = MoltObject::from_int(compiled.compiled.group_count as i64).bits();
        let flags_bits_out = MoltObject::from_int(compiled.compiled.flags).bits();
        let warn_bits = match compiled.compiled.warn_pos {
            Some(pos) => MoltObject::from_int(pos).bits(),
            None => MoltObject::none().bits(),
        };

        let tuple_ptr = alloc_tuple(
            _py,
            &[groups_bits, dictionary.bits(), flags_bits_out, warn_bits],
        );
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}
