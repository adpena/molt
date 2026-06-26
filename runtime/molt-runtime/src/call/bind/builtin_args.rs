use super::*;

pub(super) unsafe fn bind_builtin_call(
    _py: &PyToken<'_>,
    func_bits: u64,
    func_ptr: *mut u8,
    args: &CallArgs,
) -> Option<Vec<u64>> {
    unsafe {
        let fn_ptr = function_fn_ptr(func_ptr);
        if fn_ptr == fn_addr!(crate::builtins::exceptions::molt_exception_init)
            || fn_ptr == fn_addr!(crate::builtins::exceptions::molt_exception_new_bound)
        {
            return bind_builtin_exception_args(_py, args);
        }
        if callable_matches_runtime_symbol(
            Some(MoltObject::from_ptr(func_ptr).bits()),
            fn_addr!(molt_object_init),
        ) || callable_matches_runtime_symbol(
            Some(MoltObject::from_ptr(func_ptr).bits()),
            fn_addr!(molt_object_init_subclass),
        ) {
            let self_bits = args
                .pos
                .first()
                .copied()
                .unwrap_or_else(|| MoltObject::none().bits());
            return Some(vec![self_bits]);
        }
        if callable_matches_runtime_symbol(
            Some(MoltObject::from_ptr(func_ptr).bits()),
            fn_addr!(molt_object_new_bound),
        ) {
            let self_bits = args
                .pos
                .first()
                .copied()
                .unwrap_or_else(|| MoltObject::none().bits());
            return Some(vec![self_bits]);
        }
        if fn_ptr == fn_addr!(molt_int_new) {
            return bind_builtin_int_new(_py, args);
        }
        if fn_ptr == fn_addr!(molt_int_to_bytes) {
            return bind_builtin_int_bytes_codec(_py, args, "length", "byteorder");
        }
        if fn_ptr == fn_addr!(molt_int_from_bytes) {
            return bind_builtin_int_bytes_codec(_py, args, "bytes", "byteorder");
        }
        if fn_ptr == fn_addr!(molt_open_builtin) {
            return bind_builtin_open(_py, args);
        }
        if fn_ptr == fn_addr!(crate::object::ops_builtins::molt_print_builtin) {
            return bind_builtin_print(_py, args);
        }
        if fn_ptr == fn_addr!(molt_type_new) || fn_ptr == fn_addr!(molt_type_init) {
            if matches!(
                std::env::var("MOLT_TRACE_TYPE_NEW_INIT").ok().as_deref(),
                Some("1")
            ) {
                let kind = if fn_ptr == fn_addr!(molt_type_new) {
                    "type.__new__"
                } else {
                    "type.__init__"
                };
                let self_bits = args.pos.first().copied().unwrap_or(0);
                let mut meta_label = "<unknown>".to_string();
                let self_label = if let Some(self_ptr) = obj_from_bits(self_bits).as_ptr() {
                    let self_type_id = object_type_id(self_ptr);
                    if self_type_id == TYPE_ID_TYPE {
                        let label = string_obj_to_owned(obj_from_bits(class_name_bits(self_ptr)))
                            .unwrap_or_else(|| "<type>".to_string());
                        let meta_bits = object_class_bits(self_ptr);
                        if meta_bits != 0
                            && let Some(meta_ptr) = obj_from_bits(meta_bits).as_ptr()
                            && object_type_id(meta_ptr) == TYPE_ID_TYPE
                        {
                            meta_label =
                                string_obj_to_owned(obj_from_bits(class_name_bits(meta_ptr)))
                                    .unwrap_or_else(|| "<meta>".to_string());
                        }
                        label
                    } else {
                        format!("<type_id={self_type_id}>")
                    }
                } else {
                    type_name(_py, obj_from_bits(self_bits)).to_string()
                };
                eprintln!(
                    "molt bind: {} self={} meta={} pos_len={} kw_len={}",
                    kind,
                    self_label,
                    meta_label,
                    args.pos.len(),
                    args.kw_names.len(),
                );
                if matches!(
                    std::env::var("MOLT_TRACE_TYPE_NEW_INIT_BT").ok().as_deref(),
                    Some("1")
                ) {
                    eprintln!("{:?}", std::backtrace::Backtrace::force_capture());
                }
            }
            return bind_builtin_type_new_init(_py, args);
        }
        if fn_ptr == fn_addr!(dict_get_method) {
            return bind_builtin_keywords(
                _py,
                args,
                &["key", "default"],
                Some(MoltObject::none().bits()),
                None,
            );
        }
        if fn_ptr == fn_addr!(dict_setdefault_method) {
            return bind_builtin_keywords(
                _py,
                args,
                &["key", "default"],
                Some(MoltObject::none().bits()),
                None,
            );
        }
        if fn_ptr == fn_addr!(dict_fromkeys_method) {
            return bind_builtin_keywords(
                _py,
                args,
                &["iterable", "value"],
                Some(MoltObject::none().bits()),
                None,
            );
        }
        if fn_ptr == fn_addr!(dict_update_method) {
            return bind_builtin_keywords(_py, args, &["other"], Some(missing_bits(_py)), None);
        }
        if fn_ptr == fn_addr!(molt_dict_pop_method) {
            return bind_builtin_keywords(
                _py,
                args,
                &["key", "default"],
                Some(missing_bits(_py)),
                None,
            );
        }
        if fn_ptr == fn_addr!(molt_list_sort) {
            return bind_builtin_list_sort(_py, args);
        }
        if fn_ptr == fn_addr!(molt_list_pop) {
            return bind_builtin_list_pop(_py, args);
        }
        if fn_ptr == fn_addr!(molt_bytearray_pop) {
            return bind_builtin_list_pop(_py, args);
        }
        if fn_ptr == fn_addr!(molt_list_index_range) || fn_ptr == fn_addr!(molt_tuple_index_range) {
            return bind_builtin_list_index_range(_py, args);
        }
        if fn_ptr == fn_addr!(molt_string_find_slice) {
            return bind_builtin_string_find(_py, args, "find");
        }
        if fn_ptr == fn_addr!(molt_string_rfind_slice) {
            return bind_builtin_string_find(_py, args, "rfind");
        }
        if fn_ptr == fn_addr!(molt_string_index_slice)
            || fn_ptr == fn_addr!(molt_bytes_index_slice)
            || fn_ptr == fn_addr!(molt_bytearray_index_slice)
        {
            return bind_builtin_string_find(_py, args, "index");
        }
        if fn_ptr == fn_addr!(molt_string_rindex_slice)
            || fn_ptr == fn_addr!(molt_bytes_rindex_slice)
            || fn_ptr == fn_addr!(molt_bytearray_rindex_slice)
        {
            return bind_builtin_string_find(_py, args, "rindex");
        }
        if fn_ptr == fn_addr!(molt_bytes_find_slice)
            || fn_ptr == fn_addr!(molt_bytearray_find_slice)
        {
            return bind_builtin_string_find(_py, args, "find");
        }
        if fn_ptr == fn_addr!(molt_bytes_rfind_slice)
            || fn_ptr == fn_addr!(molt_bytearray_rfind_slice)
        {
            return bind_builtin_string_find(_py, args, "rfind");
        }
        if fn_ptr == fn_addr!(molt_string_split_max)
            || fn_ptr == fn_addr!(molt_bytes_split_max)
            || fn_ptr == fn_addr!(molt_bytearray_split_max)
        {
            return bind_builtin_split(_py, args, "split");
        }
        if fn_ptr == fn_addr!(molt_string_rsplit_max)
            || fn_ptr == fn_addr!(molt_bytes_rsplit_max)
            || fn_ptr == fn_addr!(molt_bytearray_rsplit_max)
        {
            return bind_builtin_split(_py, args, "rsplit");
        }
        if fn_ptr == fn_addr!(molt_string_count_slice)
            || fn_ptr == fn_addr!(molt_bytes_count_slice)
            || fn_ptr == fn_addr!(molt_bytearray_count_slice)
        {
            return bind_builtin_count(_py, args, "count");
        }
        if fn_ptr == fn_addr!(molt_string_startswith_slice) {
            return bind_builtin_prefix_check(_py, args, "startswith", "prefix");
        }
        if fn_ptr == fn_addr!(molt_string_endswith_slice) {
            return bind_builtin_prefix_check(_py, args, "endswith", "suffix");
        }
        if fn_ptr == fn_addr!(molt_bytes_startswith_slice)
            || fn_ptr == fn_addr!(molt_bytearray_startswith_slice)
        {
            return bind_builtin_prefix_check(_py, args, "startswith", "prefix");
        }
        if fn_ptr == fn_addr!(molt_bytes_endswith_slice)
            || fn_ptr == fn_addr!(molt_bytearray_endswith_slice)
        {
            return bind_builtin_prefix_check(_py, args, "endswith", "suffix");
        }
        if fn_ptr == fn_addr!(molt_bytes_hex)
            || fn_ptr == fn_addr!(molt_bytearray_hex)
            || fn_ptr == fn_addr!(molt_memoryview_hex)
        {
            return bind_builtin_bytes_hex(_py, args);
        }
        if fn_ptr == fn_addr!(molt_string_format_method) {
            return bind_builtin_string_format(_py, args);
        }
        if fn_ptr == fn_addr!(molt_string_splitlines)
            || fn_ptr == fn_addr!(molt_bytes_splitlines)
            || fn_ptr == fn_addr!(molt_bytearray_splitlines)
        {
            return bind_builtin_splitlines(_py, args);
        }
        if fn_ptr == fn_addr!(molt_set_union_multi) {
            return bind_builtin_set_multi(_py, args, "union", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_union_multi) {
            return bind_builtin_set_multi(_py, args, "union", "frozenset", TYPE_ID_FROZENSET);
        }
        if fn_ptr == fn_addr!(molt_set_intersection_multi) {
            return bind_builtin_set_multi(_py, args, "intersection", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_intersection_multi) {
            return bind_builtin_set_multi(
                _py,
                args,
                "intersection",
                "frozenset",
                TYPE_ID_FROZENSET,
            );
        }
        if fn_ptr == fn_addr!(molt_set_difference_multi) {
            return bind_builtin_set_multi(_py, args, "difference", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_difference_multi) {
            return bind_builtin_set_multi(_py, args, "difference", "frozenset", TYPE_ID_FROZENSET);
        }
        if fn_ptr == fn_addr!(molt_set_update_multi) {
            return bind_builtin_set_multi(_py, args, "update", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_set_intersection_update_multi) {
            return bind_builtin_set_multi(_py, args, "intersection_update", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_set_difference_update_multi) {
            return bind_builtin_set_multi(_py, args, "difference_update", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_set_symmetric_difference) {
            return bind_builtin_set_single(_py, args, "symmetric_difference", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_symmetric_difference) {
            return bind_builtin_set_single(
                _py,
                args,
                "symmetric_difference",
                "frozenset",
                TYPE_ID_FROZENSET,
            );
        }
        if fn_ptr == fn_addr!(molt_set_symmetric_difference_update) {
            return bind_builtin_set_single(
                _py,
                args,
                "symmetric_difference_update",
                "set",
                TYPE_ID_SET,
            );
        }
        if fn_ptr == fn_addr!(molt_set_isdisjoint) {
            return bind_builtin_set_single(_py, args, "isdisjoint", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_isdisjoint) {
            return bind_builtin_set_single(
                _py,
                args,
                "isdisjoint",
                "frozenset",
                TYPE_ID_FROZENSET,
            );
        }
        if fn_ptr == fn_addr!(molt_set_issubset) {
            return bind_builtin_set_single(_py, args, "issubset", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_issubset) {
            return bind_builtin_set_single(_py, args, "issubset", "frozenset", TYPE_ID_FROZENSET);
        }
        if fn_ptr == fn_addr!(molt_set_issuperset) {
            return bind_builtin_set_single(_py, args, "issuperset", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_issuperset) {
            return bind_builtin_set_single(
                _py,
                args,
                "issuperset",
                "frozenset",
                TYPE_ID_FROZENSET,
            );
        }
        if fn_ptr == fn_addr!(molt_set_copy_method) {
            return bind_builtin_set_noargs(_py, args, "copy", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_frozenset_copy_method) {
            return bind_builtin_set_noargs(_py, args, "copy", "frozenset", TYPE_ID_FROZENSET);
        }
        if fn_ptr == fn_addr!(molt_set_clear) {
            return bind_builtin_set_noargs(_py, args, "clear", "set", TYPE_ID_SET);
        }
        if fn_ptr == fn_addr!(molt_string_encode) {
            return bind_builtin_text_codec(_py, args, "encode");
        }
        if fn_ptr == fn_addr!(molt_bytes_decode) || fn_ptr == fn_addr!(molt_bytearray_decode) {
            return bind_builtin_text_codec(_py, args, "decode");
        }
        if fn_ptr == fn_addr!(molt_memoryview_cast) {
            return bind_builtin_memoryview_cast(_py, args);
        }
        if fn_ptr == fn_addr!(molt_file_reconfigure) {
            return bind_builtin_file_reconfigure(_py, args);
        }

        if !args.kw_names.is_empty() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "keywords are not supported for this builtin",
            );
        }

        let mut out = args.pos.clone();
        let arity = function_arity(func_ptr) as usize;
        if fn_ptr == fn_addr!(molt_bytes_maketrans) && out.len() != 2 {
            let msg = format!("maketrans expected 2 arguments, got {}", out.len());
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if out.len() > arity {
            return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
        }
        let missing = arity - out.len();
        if missing == 0 {
            return Some(out);
        }
        // Consult __defaults__ tuple stored on the function.
        // This handles user-defined functions with keyword default arguments
        // that end up in the builtin bind path (e.g. on WASM when
        // __molt_arg_names__ is not found on the function object).
        let defaults_bits = function_attr_bits(
            _py,
            func_ptr,
            intern_static_name(
                _py,
                &runtime_state(_py).interned.defaults_name,
                b"__defaults__",
            ),
        );
        if let Some(dbits) = defaults_bits
            && !obj_from_bits(dbits).is_none()
            && let Some(def_ptr) = obj_from_bits(dbits).as_ptr()
            && object_type_id(def_ptr) == TYPE_ID_TUPLE
        {
            let defaults = seq_vec_ref(def_ptr);
            let n_defaults = defaults.len();
            if missing <= n_defaults {
                // The defaults tuple covers the last n_defaults
                // parameters.  We need the last `missing` entries.
                let start = n_defaults - missing;
                out.extend(defaults.iter().take(n_defaults).skip(start).copied());
                return Some(out);
            }
        }

        // Diagnostic: log what function failed to bind
        if std::env::var("MOLT_DEBUG_BIND").is_ok() {
            let func_name = crate::type_name(_py, molt_obj_model::MoltObject::from_bits(func_bits));
            eprintln!(
                "[bind] missing required arguments: func={} pos_given={} missing={}",
                func_name,
                args.pos.len(),
                missing,
            );
        }
        raise_exception::<_>(_py, "TypeError", "missing required arguments")
    }
}

unsafe fn bind_builtin_exception_args(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required arguments");
        }
        if !args.kw_names.is_empty() {
            let head = args.pos[0];
            let head_obj = obj_from_bits(head);
            let Some(head_ptr) = head_obj.as_ptr() else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "keywords are not supported for this builtin",
                );
            };
            let allow_kw = match object_type_id(head_ptr) {
                TYPE_ID_TYPE => true,
                TYPE_ID_EXCEPTION => {
                    let oserror_bits = exception_type_bits_from_name(_py, "OSError");
                    issubclass_bits(exception_class_bits(head_ptr), oserror_bits)
                }
                _ => false,
            };
            if !allow_kw {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "keywords are not supported for this builtin",
                );
            }
        }
        let head = args.pos[0];
        let rest = &args.pos[1..];
        let tuple_ptr = alloc_tuple(_py, rest);
        if tuple_ptr.is_null() {
            return None;
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        Some(vec![head, tuple_bits])
    }
}

unsafe fn bind_builtin_int_new(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'cls'");
    }
    if args.pos.len() > 3 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let cls_bits = args.pos[0];
    let mut value_bits = args.pos.get(1).copied();
    let mut base_bits = args.pos.get(2).copied();
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "x" => {
                if value_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                value_bits = Some(val_bits);
            }
            "base" => {
                if base_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                base_bits = Some(val_bits);
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    let value_bits = value_bits.unwrap_or_else(|| MoltObject::from_int(0).bits());
    let base_bits = base_bits.unwrap_or_else(|| missing_bits(_py));
    Some(vec![cls_bits, value_bits, base_bits])
}

pub(super) unsafe fn bind_builtin_dict_update(_py: &PyToken<'_>, args: &CallArgs) -> u64 {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
        }
        let positional = args.pos.len().saturating_sub(1);
        if positional > 1 {
            let msg = format!("update expected at most 1 argument, got {}", positional);
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let dict_bits = args.pos[0];
        if positional == 1 {
            let other_bits = args.pos[1];
            let dict_obj = obj_from_bits(dict_bits);
            if let Some(dict_ptr) = dict_obj.as_ptr() {
                if object_type_id(dict_ptr) == TYPE_ID_DICT {
                    let _ = dict_update_apply(_py, dict_bits, dict_update_set_in_place, other_bits);
                } else {
                    let _ =
                        dict_update_apply(_py, dict_bits, dict_update_set_via_store, other_bits);
                }
            } else {
                let _ = dict_update_apply(_py, dict_bits, dict_update_set_via_store, other_bits);
            }
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
        }
        if !args.kw_names.is_empty() {
            for (name_bits, val_bits) in args
                .kw_names
                .iter()
                .copied()
                .zip(args.kw_values.iter().copied())
            {
                let name_obj = obj_from_bits(name_bits);
                let Some(name_ptr) = name_obj.as_ptr() else {
                    return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
                };
                if object_type_id(name_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
                }
                dict_update_set_via_store(_py, dict_bits, name_bits, val_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        MoltObject::none().bits()
    }
}

fn default_open_mode_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &runtime_state(_py).special_cache.open_default_mode,
        || {
            let ptr = alloc_string(_py, b"r");
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        },
    )
}

unsafe fn bind_builtin_bytes_hex(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 3 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let self_bits = args.pos[0];
    let mut sep_bits = args.pos.get(1).copied();
    let mut bytes_per_sep_bits = args.pos.get(2).copied();
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "sep" => {
                if sep_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                sep_bits = Some(val_bits);
            }
            "bytes_per_sep" => {
                if bytes_per_sep_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                bytes_per_sep_bits = Some(val_bits);
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    let sep_bits = sep_bits.unwrap_or_else(|| missing_bits(_py));
    let bytes_per_sep_bits = bytes_per_sep_bits.unwrap_or_else(|| missing_bits(_py));
    Some(vec![self_bits, sep_bits, bytes_per_sep_bits])
}

unsafe fn bind_builtin_keywords(
    _py: &PyToken<'_>,
    args: &CallArgs,
    names: &[&str],
    default_bits: Option<u64>,
    extra_bits: Option<u64>,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let mut out = vec![args.pos[0]];
    let mut values: Vec<Option<u64>> = vec![None; names.len()];
    let mut pos_idx = 1usize;
    while pos_idx < args.pos.len() {
        let idx = pos_idx - 1;
        if idx >= names.len() {
            return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
        }
        values[idx] = Some(args.pos[pos_idx]);
        pos_idx += 1;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in names.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("got an unexpected keyword '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    for (idx, val) in values.iter_mut().enumerate() {
        if val.is_none() {
            if let Some(bits) = default_bits {
                *val = Some(bits);
                continue;
            }
            let name = names[idx];
            let msg = format!("missing required argument '{name}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    for val in values.into_iter().flatten() {
        out.push(val);
    }
    if let Some(extra) = extra_bits {
        out.push(extra);
    }
    Some(out)
}

unsafe fn bind_builtin_int_bytes_codec(
    _py: &PyToken<'_>,
    args: &CallArgs,
    required_0: &str,
    required_1: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 4 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut first_bits = args.pos.get(1).copied();
    let mut second_bits = args.pos.get(2).copied();
    let mut signed_bits = args.pos.get(3).copied();
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            _ if name_str == required_0 => {
                if first_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                first_bits = Some(val_bits);
            }
            _ if name_str == required_1 => {
                if second_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                second_bits = Some(val_bits);
            }
            "signed" => {
                if signed_bits.is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                signed_bits = Some(val_bits);
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    let Some(first_bits) = first_bits else {
        let msg = format!("missing required argument '{required_0}'");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    let Some(second_bits) = second_bits else {
        let msg = format!("missing required argument '{required_1}'");
        return raise_exception::<_>(_py, "TypeError", &msg);
    };
    let signed_bits = signed_bits.unwrap_or_else(|| MoltObject::from_bool(false).bits());
    Some(vec![args.pos[0], first_bits, second_bits, signed_bits])
}

pub(super) unsafe fn bind_builtin_class_text_io_wrapper(
    _py: &PyToken<'_>,
    args: &CallArgs,
) -> Option<Vec<u64>> {
    const NAMES: [&str; 6] = [
        "buffer",
        "encoding",
        "errors",
        "newline",
        "line_buffering",
        "write_through",
    ];
    if args.pos.len() > NAMES.len() {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut values: Vec<Option<u64>> = vec![None; NAMES.len()];
    for (idx, &val) in args.pos.iter().enumerate() {
        values[idx] = Some(val);
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in NAMES.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("got an unexpected keyword '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    if values[0].is_none() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'buffer'");
    }
    for slot in values.iter_mut().take(4).skip(1) {
        if slot.is_none() {
            *slot = Some(MoltObject::none().bits());
        }
    }
    for slot in values.iter_mut().take(6).skip(4) {
        if slot.is_none() {
            *slot = Some(MoltObject::from_bool(false).bits());
        }
    }
    Some(values.into_iter().flatten().collect())
}

pub(super) unsafe fn bind_builtin_class_string_io(
    _py: &PyToken<'_>,
    args: &CallArgs,
) -> Option<Vec<u64>> {
    const NAMES: [&str; 2] = ["initial_value", "newline"];
    if args.pos.len() > NAMES.len() {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut values: [Option<u64>; 2] = [None; 2];
    for (idx, &val) in args.pos.iter().enumerate() {
        values[idx] = Some(val);
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in NAMES.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("got an unexpected keyword '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    if values[0].is_none() {
        values[0] = Some(MoltObject::none().bits());
    }
    if values[1].is_none() {
        values[1] = Some(MoltObject::none().bits());
    }
    Some(values.into_iter().flatten().collect())
}

/// Bind `print(*args, sep=' ', end='\n', file=None, flush=False)`.
///
/// `molt_print_builtin` takes 5 positional C params:
///   (args_tuple, sep, end, file, flush)
/// The first param is a tuple of the `*args` vararg.
unsafe fn bind_builtin_print(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    // Build the *args tuple from positional arguments.
    let args_ptr = crate::object::builders::alloc_tuple(_py, &args.pos);
    let args_tuple = MoltObject::from_ptr(args_ptr).bits();
    // Keyword-only defaults.
    let default_sep = crate::object::builders::alloc_string(_py, b" ");
    let default_end = crate::object::builders::alloc_string(_py, b"\n");
    let sep_default = MoltObject::from_ptr(default_sep).bits();
    let end_default = MoltObject::from_ptr(default_end).bits();
    let file_default = MoltObject::none().bits();
    let flush_default = MoltObject::from_bool(false).bits();
    let mut sep = sep_default;
    let mut end = end_default;
    let mut file = file_default;
    let mut flush = flush_default;
    // Match keywords.
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_default();
        match name_str.as_str() {
            "sep" => sep = val_bits,
            "end" => end = val_bits,
            "file" => file = val_bits,
            "flush" => flush = val_bits,
            _ => {
                let msg = format!("'{}' is an invalid keyword argument for print", name_str);
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    Some(vec![args_tuple, sep, end, file, flush])
}

pub(super) unsafe fn bind_builtin_open(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    const NAMES: [&str; 8] = [
        "file",
        "mode",
        "buffering",
        "encoding",
        "errors",
        "newline",
        "closefd",
        "opener",
    ];
    let mut values: [Option<u64>; 8] = [None; 8];
    for (idx, val) in args.pos.iter().copied().enumerate() {
        if idx >= values.len() {
            let msg = format!(
                "open() takes at most 8 arguments ({} given)",
                args.pos.len()
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        values[idx] = Some(val);
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let mut matched = false;
        for (idx, expected) in NAMES.iter().enumerate() {
            if name_str == *expected {
                if values[idx].is_some() {
                    let msg = if idx < args.pos.len() {
                        format!(
                            "argument for open() given by name ('{name_str}') and position ({})",
                            idx + 1
                        )
                    } else {
                        format!("open() got multiple values for argument '{name_str}'")
                    };
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
                matched = true;
                break;
            }
        }
        if !matched {
            let msg = format!("open() got an unexpected keyword argument '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
    }
    if values[0].is_none() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "open() missing required argument 'file' (pos 1)",
        );
    }
    if values[1].is_none() {
        let mode_bits = default_open_mode_bits(_py);
        if obj_from_bits(mode_bits).is_none() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        values[1] = Some(mode_bits);
    }
    if values[2].is_none() {
        values[2] = Some(MoltObject::from_int(-1).bits());
    }
    if values[3].is_none() {
        values[3] = Some(MoltObject::none().bits());
    }
    if values[4].is_none() {
        values[4] = Some(MoltObject::none().bits());
    }
    if values[5].is_none() {
        values[5] = Some(MoltObject::none().bits());
    }
    if values[6].is_none() {
        values[6] = Some(MoltObject::from_bool(true).bits());
    }
    if values[7].is_none() {
        values[7] = Some(MoltObject::none().bits());
    }
    let mut out = Vec::with_capacity(values.len());
    for val in values {
        out.push(val.unwrap());
    }
    Some(out)
}

unsafe fn bind_builtin_type_new_init(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'cls'");
        }
        let mut values: [Option<u64>; 3] = [None, None, None];
        for (idx, val) in args.pos.iter().copied().enumerate().skip(1) {
            let pos_idx = idx - 1;
            if pos_idx >= values.len() {
                return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
            }
            values[pos_idx] = Some(val);
        }
        let mut extra_pairs: Vec<u64> = Vec::new();
        for (name_bits, val_bits) in args
            .kw_names
            .iter()
            .copied()
            .zip(args.kw_values.iter().copied())
        {
            let name_obj = obj_from_bits(name_bits);
            let Some(name_ptr) = name_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            };
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            }
            let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
            let slot = match name_str.as_str() {
                "name" => Some(0usize),
                "bases" => Some(1usize),
                "dict" | "namespace" => Some(2usize),
                _ => None,
            };
            if let Some(idx) = slot {
                if values[idx].is_some() {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                values[idx] = Some(val_bits);
            } else {
                extra_pairs.push(name_bits);
                extra_pairs.push(val_bits);
            }
        }
        let names = ["name", "bases", "dict"];
        for (idx, val) in values.iter().enumerate() {
            if val.is_none() {
                if matches!(
                    std::env::var("MOLT_TRACE_TYPE_NEW_INIT").ok().as_deref(),
                    Some("1")
                ) {
                    eprintln!(
                        "molt bind: type.__new__/__init__ missing {} (pos_len={} kw_len={})",
                        names[idx],
                        args.pos.len(),
                        args.kw_names.len(),
                    );
                }
                let msg = format!("missing required argument '{}'", names[idx]);
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
        let mut out = vec![args.pos[0]];
        for val in values.into_iter().flatten() {
            out.push(val);
        }
        if extra_pairs.is_empty() {
            out.push(MoltObject::none().bits());
            return Some(out);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &extra_pairs);
        if dict_ptr.is_null() {
            return Some(out);
        }
        out.push(MoltObject::from_ptr(dict_ptr).bits());
        Some(out)
    }
}

unsafe fn bind_builtin_list_sort(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 1 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut key_bits = MoltObject::none().bits();
    let mut reverse_bits = MoltObject::from_bool(false).bits();
    let mut key_set = false;
    let mut reverse_set = false;
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "key" => {
                if key_set {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                key_bits = val_bits;
                key_set = true;
            }
            "reverse" => {
                if reverse_set {
                    let msg = format!("got multiple values for argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                reverse_bits = val_bits;
                reverse_set = true;
            }
            _ => {
                let msg = format!("got an unexpected keyword '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    Some(vec![args.pos[0], key_bits, reverse_bits])
}

unsafe fn bind_builtin_list_pop(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if !args.kw_names.is_empty() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "keywords are not supported for this builtin",
        );
    }
    if args.pos.len() > 2 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut out = args.pos.clone();
    if out.len() == 1 {
        out.push(MoltObject::none().bits());
    }
    Some(out)
}

unsafe fn bind_builtin_list_index_range(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if !args.kw_names.is_empty() {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "keywords are not supported for this builtin",
        );
    }
    if args.pos.len() < 2 {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'value'");
    }
    if args.pos.len() > 4 {
        return raise_exception::<_>(_py, "TypeError", "too many positional arguments");
    }
    let mut out = args.pos.clone();
    let missing = missing_bits(_py);
    if out.len() == 2 {
        out.push(missing);
        out.push(missing);
    } else if out.len() == 3 {
        out.push(missing);
    }
    Some(out)
}

unsafe fn bind_builtin_string_find(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 3 {
        let msg = format!(
            "{func_name}() takes at most 3 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut needle_bits: Option<u64> = None;
    let mut start_bits: Option<u64> = None;
    let mut end_bits: Option<u64> = None;
    let mut saw_sub = false;
    let mut saw_start = false;
    let mut saw_end = false;
    if positional >= 1 {
        needle_bits = Some(args.pos[1]);
        saw_sub = true;
    }
    if positional >= 2 {
        start_bits = Some(args.pos[2]);
        saw_start = true;
    }
    if positional >= 3 {
        end_bits = Some(args.pos[3]);
        saw_end = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "sub" => (&mut needle_bits, &mut saw_sub),
            "start" => (&mut start_bits, &mut saw_start),
            "end" => (&mut end_bits, &mut saw_end),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let needle_bits = needle_bits.unwrap_or_else(|| {
        raise_exception::<_>(_py, "TypeError", "missing required argument 'sub'")
    });
    let start_bits = start_bits.unwrap_or_else(|| MoltObject::none().bits());
    let end_bits = end_bits.unwrap_or_else(|| MoltObject::none().bits());
    let has_start_bits = MoltObject::from_bool(saw_start).bits();
    let has_end_bits = MoltObject::from_bool(saw_end).bits();
    Some(vec![
        args.pos[0],
        needle_bits,
        start_bits,
        end_bits,
        has_start_bits,
        has_end_bits,
    ])
}

unsafe fn bind_builtin_count(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 3 {
        let msg = format!(
            "{func_name}() takes at most 3 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut needle_bits: Option<u64> = None;
    let mut start_bits: Option<u64> = None;
    let mut end_bits: Option<u64> = None;
    let mut saw_sub = false;
    let mut saw_start = false;
    let mut saw_end = false;
    if positional >= 1 {
        needle_bits = Some(args.pos[1]);
        saw_sub = true;
    }
    if positional >= 2 {
        start_bits = Some(args.pos[2]);
        saw_start = true;
    }
    if positional >= 3 {
        end_bits = Some(args.pos[3]);
        saw_end = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "sub" => (&mut needle_bits, &mut saw_sub),
            "start" => (&mut start_bits, &mut saw_start),
            "end" => (&mut end_bits, &mut saw_end),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let needle_bits = needle_bits.unwrap_or_else(|| {
        raise_exception::<_>(_py, "TypeError", "missing required argument 'sub'")
    });
    let start_bits = start_bits.unwrap_or_else(|| MoltObject::none().bits());
    let end_bits = end_bits.unwrap_or_else(|| MoltObject::none().bits());
    let has_start_bits = MoltObject::from_bool(saw_start).bits();
    let has_end_bits = MoltObject::from_bool(saw_end).bits();
    Some(vec![
        args.pos[0],
        needle_bits,
        start_bits,
        end_bits,
        has_start_bits,
        has_end_bits,
    ])
}

unsafe fn bind_builtin_split(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 2 {
        let msg = format!(
            "{func_name}() takes at most 2 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut sep_bits: Option<u64> = None;
    let mut maxsplit_bits: Option<u64> = None;
    let mut saw_sep = false;
    let mut saw_maxsplit = false;
    if positional >= 1 {
        sep_bits = Some(args.pos[1]);
        saw_sep = true;
    }
    if positional >= 2 {
        maxsplit_bits = Some(args.pos[2]);
        saw_maxsplit = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "sep" => (&mut sep_bits, &mut saw_sep),
            "maxsplit" => (&mut maxsplit_bits, &mut saw_maxsplit),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let sep_bits = sep_bits.unwrap_or_else(|| MoltObject::none().bits());
    let maxsplit_bits = maxsplit_bits.unwrap_or_else(|| MoltObject::from_int(-1).bits());
    Some(vec![args.pos[0], sep_bits, maxsplit_bits])
}

unsafe fn bind_builtin_splitlines(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 1 {
        let msg = format!(
            "splitlines() takes at most 1 argument ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut keepends_bits: Option<u64> = None;
    let mut saw_keepends = false;
    if positional == 1 {
        keepends_bits = Some(args.pos[1]);
        saw_keepends = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        if name_str != "keepends" {
            // CPython 3.13 changed str/bytes/bytearray.splitlines' invalid-kwarg
            // TypeError to the generic "got an unexpected keyword argument" form;
            // 3.12 used the specific "is an invalid keyword argument for
            // splitlines()" form. Gate on the configured target version so output
            // matches the emulated CPython across 3.12/3.13/3.14 on every arch/OS.
            let msg = if crate::object::ops_sys::runtime_target_at_least(_py, 3, 13) {
                format!("splitlines() got an unexpected keyword argument '{name_str}'")
            } else {
                format!("'{name_str}' is an invalid keyword argument for splitlines()")
            };
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if saw_keepends {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "splitlines() got multiple values for argument 'keepends'",
            );
        }
        keepends_bits = Some(val_bits);
        saw_keepends = true;
    }
    let keepends_bits = keepends_bits.unwrap_or_else(|| MoltObject::from_bool(false).bits());
    Some(vec![args.pos[0], keepends_bits])
}

unsafe fn bind_builtin_set_multi(
    _py: &PyToken<'_>,
    args: &CallArgs,
    method: &str,
    owner_name: &str,
    owner_type_id: u32,
) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
        }
        let self_obj = obj_from_bits(args.pos[0]);
        let mut is_owner = false;
        if let Some(self_ptr) = self_obj.as_ptr() {
            is_owner = object_type_id(self_ptr) == owner_type_id;
        }
        if !is_owner {
            let msg = format!(
                "descriptor '{method}' for '{owner_name}' objects doesn't apply to a '{}' object",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if !args.kw_names.is_empty() {
            let msg = format!(
                "{}.{method}() takes no keyword arguments",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let tuple_ptr = alloc_tuple(_py, &args.pos[1..]);
        if tuple_ptr.is_null() {
            return None;
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        Some(vec![args.pos[0], tuple_bits])
    }
}

unsafe fn bind_builtin_set_single(
    _py: &PyToken<'_>,
    args: &CallArgs,
    method: &str,
    owner_name: &str,
    owner_type_id: u32,
) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
        }
        let self_obj = obj_from_bits(args.pos[0]);
        let mut is_owner = false;
        if let Some(self_ptr) = self_obj.as_ptr() {
            is_owner = object_type_id(self_ptr) == owner_type_id;
        }
        if !is_owner {
            let msg = format!(
                "descriptor '{method}' for '{owner_name}' objects doesn't apply to a '{}' object",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if !args.kw_names.is_empty() {
            let msg = format!(
                "{}.{method}() takes no keyword arguments",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let positional = args.pos.len().saturating_sub(1);
        if positional != 1 {
            let msg = format!(
                "{}.{method}() takes exactly one argument ({} given)",
                type_name(_py, self_obj),
                positional
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        Some(vec![args.pos[0], args.pos[1]])
    }
}

unsafe fn bind_builtin_set_noargs(
    _py: &PyToken<'_>,
    args: &CallArgs,
    method: &str,
    owner_name: &str,
    owner_type_id: u32,
) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
        }
        let self_obj = obj_from_bits(args.pos[0]);
        let mut is_owner = false;
        if let Some(self_ptr) = self_obj.as_ptr() {
            is_owner = object_type_id(self_ptr) == owner_type_id;
        }
        if !is_owner {
            let msg = format!(
                "descriptor '{method}' for '{owner_name}' objects doesn't apply to a '{}' object",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if !args.kw_names.is_empty() {
            let msg = format!(
                "{}.{method}() takes no keyword arguments",
                type_name(_py, self_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let positional = args.pos.len().saturating_sub(1);
        if positional != 0 {
            let msg = format!(
                "{}.{method}() takes no arguments ({} given)",
                type_name(_py, self_obj),
                positional
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        Some(vec![args.pos[0]])
    }
}

unsafe fn bind_builtin_prefix_check(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
    needle_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 3 {
        let msg = format!(
            "{func_name}() takes at most 3 arguments ({} given)",
            positional
        );
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let mut needle_bits: Option<u64> = None;
    let mut start_bits: Option<u64> = None;
    let mut end_bits: Option<u64> = None;
    let mut saw_needle = false;
    let mut saw_start = false;
    let mut saw_end = false;
    if positional >= 1 {
        needle_bits = Some(args.pos[1]);
        saw_needle = true;
    }
    if positional >= 2 {
        start_bits = Some(args.pos[2]);
        saw_start = true;
    }
    if positional >= 3 {
        end_bits = Some(args.pos[3]);
        saw_end = true;
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        let target = match name_str.as_str() {
            "start" => (&mut start_bits, &mut saw_start),
            "end" => (&mut end_bits, &mut saw_end),
            _ if name_str == needle_name => (&mut needle_bits, &mut saw_needle),
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        if target.0.is_some() {
            let msg = format!(
                "{}() got multiple values for argument '{}'",
                func_name, name_str
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        *target.0 = Some(val_bits);
        *target.1 = true;
    }
    let needle_bits = needle_bits.unwrap_or_else(|| {
        let msg = format!("missing required argument '{needle_name}'");
        raise_exception::<_>(_py, "TypeError", &msg)
    });
    let start_bits = start_bits.unwrap_or_else(|| MoltObject::none().bits());
    let end_bits = end_bits.unwrap_or_else(|| MoltObject::none().bits());
    let has_start_bits = MoltObject::from_bool(saw_start).bits();
    let has_end_bits = MoltObject::from_bool(saw_end).bits();
    Some(vec![
        args.pos[0],
        needle_bits,
        start_bits,
        end_bits,
        has_start_bits,
        has_end_bits,
    ])
}

unsafe fn bind_builtin_string_format(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    unsafe {
        if args.pos.is_empty() {
            return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
        }
        let tuple_ptr = alloc_tuple(_py, &args.pos[1..]);
        if tuple_ptr.is_null() {
            return None;
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let mut pairs = Vec::with_capacity(args.kw_names.len() * 2);
        for (name_bits, val_bits) in args
            .kw_names
            .iter()
            .copied()
            .zip(args.kw_values.iter().copied())
        {
            let name_obj = obj_from_bits(name_bits);
            let Some(name_ptr) = name_obj.as_ptr() else {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            };
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "keywords must be strings");
            }
            pairs.push(name_bits);
            pairs.push(val_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            return None;
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        Some(vec![args.pos[0], tuple_bits, dict_bits])
    }
}

unsafe fn bind_builtin_memoryview_cast(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let provided = args.pos.len().saturating_sub(1);
    if provided == 0 {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "cast() missing required argument 'format' (pos 1)",
        );
    }
    if provided > 2 {
        let msg = format!("cast() takes at most 2 arguments ({provided} given)");
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let format_bits = args.pos[1];
    let mut shape_bits: Option<u64> = None;
    if provided == 2 {
        shape_bits = Some(args.pos[2]);
    }
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        if name_str != "shape" {
            let msg = format!("cast() got an unexpected keyword argument '{name_str}'");
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        if shape_bits.is_some() {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "cast() got multiple values for argument 'shape'",
            );
        }
        shape_bits = Some(val_bits);
    }
    let (shape_bits, has_shape_bits) = if let Some(bits) = shape_bits {
        (bits, MoltObject::from_bool(true).bits())
    } else {
        (
            MoltObject::none().bits(),
            MoltObject::from_bool(false).bits(),
        )
    };
    Some(vec![args.pos[0], format_bits, shape_bits, has_shape_bits])
}

unsafe fn bind_builtin_file_reconfigure(_py: &PyToken<'_>, args: &CallArgs) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    if args.pos.len() > 1 {
        return raise_exception::<_>(
            _py,
            "TypeError",
            "reconfigure() takes no positional arguments",
        );
    }
    let missing = missing_bits(_py);
    let mut encoding_bits = missing;
    let mut errors_bits = missing;
    let mut newline_bits = missing;
    let mut line_buffering_bits = missing;
    let mut write_through_bits = missing;
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "encoding" => {
                if encoding_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                encoding_bits = val_bits;
            }
            "errors" => {
                if errors_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                errors_bits = val_bits;
            }
            "newline" => {
                if newline_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                newline_bits = val_bits;
            }
            "line_buffering" => {
                if line_buffering_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                line_buffering_bits = val_bits;
            }
            "write_through" => {
                if write_through_bits != missing {
                    let msg = format!("got multiple values for keyword argument '{name_str}'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                write_through_bits = val_bits;
            }
            _ => {
                // CPython 3.13 changed TextIOWrapper.reconfigure's invalid-kwarg
                // TypeError to the generic "got an unexpected keyword argument"
                // form; 3.12 used the specific "is an invalid keyword argument
                // for reconfigure()" form. Gate on the configured target version
                // so output matches the emulated CPython across 3.12/3.13/3.14 on
                // every arch/OS (mirrors the splitlines() gating above).
                let msg = if crate::object::ops_sys::runtime_target_at_least(_py, 3, 13) {
                    format!("reconfigure() got an unexpected keyword argument '{name_str}'")
                } else {
                    format!("'{name_str}' is an invalid keyword argument for reconfigure()")
                };
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    Some(vec![
        args.pos[0],
        encoding_bits,
        errors_bits,
        newline_bits,
        line_buffering_bits,
        write_through_bits,
    ])
}

unsafe fn bind_builtin_text_codec(
    _py: &PyToken<'_>,
    args: &CallArgs,
    func_name: &str,
) -> Option<Vec<u64>> {
    if args.pos.is_empty() {
        return raise_exception::<_>(_py, "TypeError", "missing required argument 'self'");
    }
    let positional = args.pos.len().saturating_sub(1);
    if positional > 2 {
        let msg = format!("{func_name}() takes at most 2 arguments ({positional} given)");
        return raise_exception::<_>(_py, "TypeError", &msg);
    }
    let missing = missing_bits(_py);
    let mut encoding_bits = if positional >= 1 {
        args.pos[1]
    } else {
        missing
    };
    let mut errors_bits = if positional >= 2 {
        args.pos[2]
    } else {
        missing
    };
    for (name_bits, val_bits) in args
        .kw_names
        .iter()
        .copied()
        .zip(args.kw_values.iter().copied())
    {
        let name_obj = obj_from_bits(name_bits);
        let name_str = string_obj_to_owned(name_obj).unwrap_or_else(|| "?".to_string());
        match name_str.as_str() {
            "encoding" => {
                if encoding_bits != missing {
                    let msg = format!("{func_name}() got multiple values for argument 'encoding'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                encoding_bits = val_bits;
            }
            "errors" => {
                if errors_bits != missing {
                    let msg = format!("{func_name}() got multiple values for argument 'errors'");
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                errors_bits = val_bits;
            }
            _ => {
                let msg = format!("{func_name}() got an unexpected keyword argument '{name_str}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    Some(vec![args.pos[0], encoding_bits, errors_bits])
}
