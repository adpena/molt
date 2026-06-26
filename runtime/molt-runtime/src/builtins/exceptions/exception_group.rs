use super::*;

struct ExceptionGroupItems {
    items: Vec<u64>,
    all_exception: bool,
}

struct ExceptionGroupItem {
    bits: u64,
    owned: bool,
}

pub(super) fn exception_group_message_bits(_py: &PyToken<'_>, ptr: *mut u8) -> u64 {
    let dict_bits = unsafe { exception_dict_bits(ptr) };
    if !obj_from_bits(dict_bits).is_none()
        && dict_bits != 0
        && let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
    {
        unsafe {
            if object_type_id(dict_ptr) == TYPE_ID_DICT {
                let key_bits = intern_static_name(
                    _py,
                    &exceptions_state(_py).exc_group_message_name,
                    b"message",
                );
                if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, key_bits) {
                    return val_bits;
                }
            }
        }
    }
    exception_materialized_message_bits(_py, ptr)
}

pub(super) fn exception_group_exceptions_bits(_py: &PyToken<'_>, ptr: *mut u8) -> Option<u64> {
    let dict_bits = unsafe { exception_dict_bits(ptr) };
    if obj_from_bits(dict_bits).is_none() || dict_bits == 0 {
        return None;
    }
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    unsafe {
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let key_bits = intern_static_name(
            _py,
            &exceptions_state(_py).exc_group_exceptions_name,
            b"exceptions",
        );
        dict_get_in_place(_py, dict_ptr, key_bits)
    }
}

fn exception_group_collect_exceptions(
    _py: &PyToken<'_>,
    exceptions_bits: u64,
) -> Option<ExceptionGroupItems> {
    let builtins = builtin_classes(_py);
    let mut items: Vec<u64> = Vec::new();
    let mut all_exception = true;
    let exceptions_obj = obj_from_bits(exceptions_bits);
    if let Some(ptr) = exceptions_obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_TUPLE || type_id == TYPE_ID_LIST {
                let elems = seq_vec_ref(ptr);
                if elems.is_empty() {
                    let _ = raise_exception::<u64>(
                        _py,
                        "ValueError",
                        "second argument (exceptions) must be a non-empty sequence",
                    );
                    return None;
                }
                for (idx, &item_bits) in elems.iter().enumerate() {
                    let item_class = type_of_bits(_py, item_bits);
                    if !issubclass_bits(item_class, builtins.base_exception) {
                        let msg = format!(
                            "Item {idx} of second argument (exceptions) is not an exception"
                        );
                        let _ = raise_exception::<u64>(_py, "ValueError", &msg);
                        return None;
                    }
                    if !issubclass_bits(item_class, builtins.exception) {
                        all_exception = false;
                    }
                    items.push(item_bits);
                }
                return Some(ExceptionGroupItems {
                    items,
                    all_exception,
                });
            }
        }
        let getitem_name = attr_name_bits_from_bytes(_py, b"__getitem__")?;
        let getitem_bits = unsafe { attr_lookup_ptr_allow_missing(_py, ptr, getitem_name) };
        dec_ref_bits(_py, getitem_name);
        if let Some(bits) = getitem_bits {
            dec_ref_bits(_py, bits);
        } else {
            let _ = raise_exception::<u64>(
                _py,
                "TypeError",
                "second argument (exceptions) must be a sequence",
            );
            return None;
        }
        let mut index = 0i64;
        loop {
            let idx_bits = MoltObject::from_int(index).bits();
            let item_bits = molt_index(exceptions_bits, idx_bits);
            if exception_pending(_py) {
                let exc_bits = molt_exception_last();
                let exc_obj = obj_from_bits(exc_bits);
                let mut is_index = false;
                if let Some(exc_ptr) = exc_obj.as_ptr() {
                    unsafe {
                        if object_type_id(exc_ptr) == TYPE_ID_EXCEPTION {
                            let kind_bits = exception_kind_bits(exc_ptr);
                            let kind = string_obj_to_owned(obj_from_bits(kind_bits));
                            if kind.as_deref() == Some("IndexError") {
                                is_index = true;
                            }
                        }
                    }
                }
                if is_index {
                    clear_exception(_py);
                    dec_ref_bits(_py, exc_bits);
                    if items.is_empty() {
                        let _ = raise_exception::<u64>(
                            _py,
                            "ValueError",
                            "second argument (exceptions) must be a non-empty sequence",
                        );
                        return None;
                    }
                    break;
                }
                dec_ref_bits(_py, exc_bits);
                return None;
            }
            let item_class = type_of_bits(_py, item_bits);
            if !issubclass_bits(item_class, builtins.base_exception) {
                let msg =
                    format!("Item {index} of second argument (exceptions) is not an exception");
                let _ = raise_exception::<u64>(_py, "ValueError", &msg);
                return None;
            }
            if !issubclass_bits(item_class, builtins.exception) {
                all_exception = false;
            }
            items.push(item_bits);
            index += 1;
        }
        return Some(ExceptionGroupItems {
            items,
            all_exception,
        });
    }
    let _ = raise_exception::<u64>(
        _py,
        "TypeError",
        "second argument (exceptions) must be a sequence",
    );
    None
}

fn exception_group_alloc(
    _py: &PyToken<'_>,
    class_bits: u64,
    message_bits: u64,
    args_exceptions_bits: u64,
    items: &[u64],
    exceptions_tuple_bits: Option<u64>,
) -> Option<u64> {
    let tuple_bits = if let Some(bits) = exceptions_tuple_bits {
        bits
    } else {
        let tuple_ptr = alloc_tuple(_py, items);
        if tuple_ptr.is_null() {
            return None;
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    };
    let args_ptr = alloc_tuple(_py, &[message_bits, args_exceptions_bits]);
    if args_ptr.is_null() {
        dec_ref_bits(_py, tuple_bits);
        return None;
    }
    let args_bits = MoltObject::from_ptr(args_ptr).bits();
    let msg_name_bits = intern_static_name(
        _py,
        &exceptions_state(_py).exc_group_message_name,
        b"message",
    );
    let exceptions_name_bits = intern_static_name(
        _py,
        &exceptions_state(_py).exc_group_exceptions_name,
        b"exceptions",
    );
    let dict_ptr = alloc_dict_with_pairs(
        _py,
        &[
            msg_name_bits,
            message_bits,
            exceptions_name_bits,
            tuple_bits,
        ],
    );
    if dict_ptr.is_null() {
        dec_ref_bits(_py, args_bits);
        dec_ref_bits(_py, tuple_bits);
        return None;
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let kind_bits = if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() {
        unsafe { class_name_bits(class_ptr) }
    } else {
        0
    };
    let ptr = alloc_exception_obj(
        _py,
        kind_bits,
        message_bits,
        class_bits,
        args_bits,
        dict_bits,
    );
    dec_ref_bits(_py, dict_bits);
    dec_ref_bits(_py, args_bits);
    dec_ref_bits(_py, tuple_bits);
    if ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(ptr).bits())
    }
}

unsafe fn exception_group_set_slot_bits(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    slot_idx: usize,
    bits: u64,
) {
    unsafe {
        let slot = ptr.add(slot_idx * std::mem::size_of::<u64>()) as *mut u64;
        let old_bits = *slot;
        if old_bits != bits {
            dec_ref_bits(_py, old_bits);
            inc_ref_bits(_py, bits);
            *slot = bits;
        }
    }
}

unsafe fn exception_group_copy_metadata(
    _py: &PyToken<'_>,
    dest_ptr: *mut u8,
    src_ptr: *mut u8,
    copy_context: bool,
    copy_trace: bool,
    suppress: bool,
    copy_notes: bool,
) {
    unsafe {
        if copy_context {
            let cause_bits = exception_cause_bits(src_ptr);
            let context_bits = exception_context_bits(src_ptr);
            exception_group_set_slot_bits(_py, dest_ptr, 2, cause_bits);
            exception_group_set_slot_bits(_py, dest_ptr, 3, context_bits);
        }
        if copy_trace {
            let trace_bits = exception_trace_bits(src_ptr);
            exception_group_set_slot_bits(_py, dest_ptr, 5, trace_bits);
        }
        let suppress_bits = MoltObject::from_bool(suppress).bits();
        exception_group_set_slot_bits(_py, dest_ptr, 4, suppress_bits);

        // Propagate __notes__ (PEP 678): shallow-copy the notes list from
        // the source exception's dict into the destination exception's dict.
        if copy_notes {
            let src_dict_bits = exception_dict_bits(src_ptr);
            if let Some(src_dict_ptr) = obj_from_bits(src_dict_bits).as_ptr()
                && object_type_id(src_dict_ptr) == TYPE_ID_DICT
            {
                let notes_name =
                    intern_static_name(_py, &runtime_state(_py).interned.notes_name, b"__notes__");
                if let Some(src_notes_bits) = dict_get_in_place(_py, src_dict_ptr, notes_name) {
                    // Shallow-copy the notes list to avoid aliasing
                    if let Some(src_notes_ptr) = obj_from_bits(src_notes_bits).as_ptr()
                        && object_type_id(src_notes_ptr) == TYPE_ID_LIST
                    {
                        let notes_elems = seq_vec_ref(src_notes_ptr);
                        let new_list_ptr = alloc_list(_py, notes_elems);
                        if !new_list_ptr.is_null() {
                            let new_list_bits = MoltObject::from_ptr(new_list_ptr).bits();
                            // Ensure dest has a dict
                            let dest_dict_bits = exception_dict_bits(dest_ptr);
                            let dest_dict_ptr =
                                if let Some(dd) = obj_from_bits(dest_dict_bits).as_ptr() {
                                    if object_type_id(dd) == TYPE_ID_DICT {
                                        dd
                                    } else {
                                        let dp = alloc_dict_with_pairs(_py, &[]);
                                        if dp.is_null() {
                                            dec_ref_bits(_py, new_list_bits);
                                            return;
                                        }
                                        let dp_bits = MoltObject::from_ptr(dp).bits();
                                        exception_group_set_slot_bits(_py, dest_ptr, 9, dp_bits);
                                        dp
                                    }
                                } else {
                                    let dp = alloc_dict_with_pairs(_py, &[]);
                                    if dp.is_null() {
                                        dec_ref_bits(_py, new_list_bits);
                                        return;
                                    }
                                    let dp_bits = MoltObject::from_ptr(dp).bits();
                                    exception_group_set_slot_bits(_py, dest_ptr, 9, dp_bits);
                                    dp
                                };
                            dict_set_in_place(_py, dest_dict_ptr, notes_name, new_list_bits);
                            dec_ref_bits(_py, new_list_bits);
                        }
                    }
                }
            }
        }
    }
}

enum ExceptionGroupMatcher {
    Type(u64),
    Callable(u64),
}

fn exception_group_parse_matcher(
    _py: &PyToken<'_>,
    matcher_bits: u64,
) -> Option<ExceptionGroupMatcher> {
    let builtins = builtin_classes(_py);
    let matcher_obj = obj_from_bits(matcher_bits);
    let Some(ptr) = matcher_obj.as_ptr() else {
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            "expected an exception type, a tuple of exception types, or a callable (other than a class)",
        );
        return None;
    };
    unsafe {
        match object_type_id(ptr) {
            TYPE_ID_TYPE => {
                if !issubclass_bits(matcher_bits, builtins.base_exception) {
                    let _ = raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "expected an exception type, a tuple of exception types, or a callable (other than a class)",
                    );
                    return None;
                }
                return Some(ExceptionGroupMatcher::Type(matcher_bits));
            }
            TYPE_ID_TUPLE => {
                let elems = seq_vec_ref(ptr);
                if elems.is_empty() {
                    let _ = raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "expected an exception type, a tuple of exception types, or a callable (other than a class)",
                    );
                    return None;
                }
                for &elem_bits in elems.iter() {
                    let Some(elem_ptr) = obj_from_bits(elem_bits).as_ptr() else {
                        let _ = raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "expected an exception type, a tuple of exception types, or a callable (other than a class)",
                        );
                        return None;
                    };
                    if object_type_id(elem_ptr) != TYPE_ID_TYPE
                        || !issubclass_bits(elem_bits, builtins.base_exception)
                    {
                        let _ = raise_exception::<u64>(
                            _py,
                            "TypeError",
                            "expected an exception type, a tuple of exception types, or a callable (other than a class)",
                        );
                        return None;
                    }
                }
                return Some(ExceptionGroupMatcher::Type(matcher_bits));
            }
            _ => {}
        }
    }
    let callable_bits = molt_is_callable(matcher_bits);
    if is_truthy(_py, obj_from_bits(callable_bits)) {
        return Some(ExceptionGroupMatcher::Callable(matcher_bits));
    }
    let _ = raise_exception::<u64>(
        _py,
        "TypeError",
        "expected an exception type, a tuple of exception types, or a callable (other than a class)",
    );
    None
}

fn exception_group_parse_except_star_matcher(_py: &PyToken<'_>, matcher_bits: u64) -> Option<u64> {
    let builtins = builtin_classes(_py);
    let matcher_obj = obj_from_bits(matcher_bits);
    let Some(ptr) = matcher_obj.as_ptr() else {
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            "catching classes that do not inherit from BaseException is not allowed",
        );
        return None;
    };
    unsafe {
        let type_id = object_type_id(ptr);
        if type_id == TYPE_ID_TYPE {
            if !issubclass_bits(matcher_bits, builtins.base_exception) {
                let _ = raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "catching classes that do not inherit from BaseException is not allowed",
                );
                return None;
            }
            if issubclass_bits(matcher_bits, builtins.base_exception_group) {
                let _ = raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "catching ExceptionGroup with except* is not allowed. Use except instead.",
                );
                return None;
            }
            return Some(matcher_bits);
        }
        if type_id == TYPE_ID_TUPLE {
            let elems = seq_vec_ref(ptr);
            if elems.is_empty() {
                let _ = raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "catching classes that do not inherit from BaseException is not allowed",
                );
                return None;
            }
            for &elem_bits in elems.iter() {
                let Some(elem_ptr) = obj_from_bits(elem_bits).as_ptr() else {
                    let _ = raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "catching classes that do not inherit from BaseException is not allowed",
                    );
                    return None;
                };
                if object_type_id(elem_ptr) != TYPE_ID_TYPE
                    || !issubclass_bits(elem_bits, builtins.base_exception)
                {
                    let _ = raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "catching classes that do not inherit from BaseException is not allowed",
                    );
                    return None;
                }
            }
            for &elem_bits in elems.iter() {
                if issubclass_bits(elem_bits, builtins.base_exception_group) {
                    let _ = raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "catching ExceptionGroup with except* is not allowed. Use except instead.",
                    );
                    return None;
                }
            }
            return Some(matcher_bits);
        }
    }
    let _ = raise_exception::<u64>(
        _py,
        "TypeError",
        "catching classes that do not inherit from BaseException is not allowed",
    );
    None
}

fn exception_group_matcher_matches(
    _py: &PyToken<'_>,
    matcher: &ExceptionGroupMatcher,
    exc_bits: u64,
) -> Option<bool> {
    match matcher {
        ExceptionGroupMatcher::Type(class_bits) => {
            Some(isinstance_bits(_py, exc_bits, *class_bits))
        }
        ExceptionGroupMatcher::Callable(call_bits) => {
            let res_bits = unsafe { call_callable1(_py, *call_bits, exc_bits) };
            if exception_pending(_py) {
                return None;
            }
            Some(is_truthy(_py, obj_from_bits(res_bits)))
        }
    }
}

fn exception_group_split_node(
    _py: &PyToken<'_>,
    exc_bits: u64,
    matcher: &ExceptionGroupMatcher,
) -> Option<(Option<ExceptionGroupItem>, Option<ExceptionGroupItem>)> {
    let exc_obj = obj_from_bits(exc_bits);
    let Some(exc_ptr) = exc_obj.as_ptr() else {
        return Some((None, None));
    };
    unsafe {
        if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
            return Some((
                None,
                Some(ExceptionGroupItem {
                    bits: exc_bits,
                    owned: false,
                }),
            ));
        }
    }
    if let Some(matches) = exception_group_matcher_matches(_py, matcher, exc_bits) {
        if matches {
            return Some((
                Some(ExceptionGroupItem {
                    bits: exc_bits,
                    owned: false,
                }),
                None,
            ));
        }
    } else {
        return None;
    }
    let class_bits = unsafe { exception_class_bits(exc_ptr) };
    let base_group_bits = builtin_classes(_py).base_exception_group;
    if !issubclass_bits(class_bits, base_group_bits) {
        return Some((
            None,
            Some(ExceptionGroupItem {
                bits: exc_bits,
                owned: false,
            }),
        ));
    }
    let Some(exceptions_bits) = exception_group_exceptions_bits(_py, exc_ptr) else {
        return Some((
            None,
            Some(ExceptionGroupItem {
                bits: exc_bits,
                owned: false,
            }),
        ));
    };
    let exceptions_obj = obj_from_bits(exceptions_bits);
    let Some(ex_ptr) = exceptions_obj.as_ptr() else {
        return Some((
            None,
            Some(ExceptionGroupItem {
                bits: exc_bits,
                owned: false,
            }),
        ));
    };
    unsafe {
        if object_type_id(ex_ptr) != TYPE_ID_TUPLE && object_type_id(ex_ptr) != TYPE_ID_LIST {
            return Some((
                None,
                Some(ExceptionGroupItem {
                    bits: exc_bits,
                    owned: false,
                }),
            ));
        }
        let elems = seq_vec_ref(ex_ptr);
        let mut match_items: Vec<ExceptionGroupItem> = Vec::new();
        let mut rest_items: Vec<ExceptionGroupItem> = Vec::new();
        for &item_bits in elems.iter() {
            let (match_part, rest_part) = exception_group_split_node(_py, item_bits, matcher)?;
            if let Some(bits) = match_part {
                match_items.push(bits);
            }
            if let Some(bits) = rest_part {
                rest_items.push(bits);
            }
        }
        let message_bits = exception_group_message_bits(_py, exc_ptr);
        let mut match_bits = None;
        let mut rest_bits = None;
        if !match_items.is_empty() {
            let match_vals: Vec<u64> = match_items.iter().map(|item| item.bits).collect();
            let list_ptr = alloc_list(_py, &match_vals);
            if list_ptr.is_null() {
                return None;
            }
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            match_bits =
                exception_group_alloc(_py, class_bits, message_bits, list_bits, &match_vals, None);
            dec_ref_bits(_py, list_bits);
            if let Some(bits) = match_bits
                && let Some(new_ptr) = obj_from_bits(bits).as_ptr()
            {
                exception_group_copy_metadata(_py, new_ptr, exc_ptr, true, true, true, true);
            }
            for item in match_items.into_iter() {
                if item.owned {
                    dec_ref_bits(_py, item.bits);
                }
            }
        }
        if !rest_items.is_empty() {
            let rest_vals: Vec<u64> = rest_items.iter().map(|item| item.bits).collect();
            let list_ptr = alloc_list(_py, &rest_vals);
            if list_ptr.is_null() {
                return None;
            }
            let list_bits = MoltObject::from_ptr(list_ptr).bits();
            rest_bits =
                exception_group_alloc(_py, class_bits, message_bits, list_bits, &rest_vals, None);
            dec_ref_bits(_py, list_bits);
            if let Some(bits) = rest_bits
                && let Some(new_ptr) = obj_from_bits(bits).as_ptr()
            {
                exception_group_copy_metadata(_py, new_ptr, exc_ptr, true, true, true, true);
            }
            for item in rest_items.into_iter() {
                if item.owned {
                    dec_ref_bits(_py, item.bits);
                }
            }
        }
        Some((
            match_bits.map(|bits| ExceptionGroupItem { bits, owned: true }),
            rest_bits.map(|bits| ExceptionGroupItem { bits, owned: true }),
        ))
    }
}

fn exception_group_make_pair_tuple(
    _py: &PyToken<'_>,
    match_item: Option<ExceptionGroupItem>,
    rest_item: Option<ExceptionGroupItem>,
) -> u64 {
    let none_bits = MoltObject::none().bits();
    let match_bits = match_item
        .as_ref()
        .map(|item| item.bits)
        .unwrap_or(none_bits);
    let rest_bits = rest_item
        .as_ref()
        .map(|item| item.bits)
        .unwrap_or(none_bits);
    let tuple_ptr = alloc_tuple(_py, &[match_bits, rest_bits]);
    if tuple_ptr.is_null() {
        if let Some(item) = match_item
            && item.owned
        {
            dec_ref_bits(_py, item.bits);
        }
        if let Some(item) = rest_item
            && item.owned
        {
            dec_ref_bits(_py, item.bits);
        }
        return MoltObject::none().bits();
    }
    if let Some(item) = match_item
        && item.owned
    {
        dec_ref_bits(_py, item.bits);
    }
    if let Some(item) = rest_item
        && item.owned
    {
        dec_ref_bits(_py, item.bits);
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

pub(super) fn alloc_exception_group_from_class_bits(
    _py: &PyToken<'_>,
    class_bits: u64,
    args_bits: u64,
) -> *mut u8 {
    let args_obj = obj_from_bits(args_bits);
    let Some(args_ptr) = args_obj.as_ptr() else {
        dec_ref_bits(_py, args_bits);
        return std::ptr::null_mut();
    };
    unsafe {
        if object_type_id(args_ptr) != TYPE_ID_TUPLE {
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        }
        let args_elems = seq_vec_ref(args_ptr);
        let argc = args_elems.len();
        if argc != 2 {
            let msg = format!(
                "BaseExceptionGroup.__new__() takes exactly 2 arguments ({} given)",
                argc
            );
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        }
        let message_bits = args_elems[0];
        let exceptions_bits = args_elems[1];
        let message_obj = obj_from_bits(message_bits);
        if let Some(msg_ptr) = message_obj.as_ptr() {
            if object_type_id(msg_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "BaseExceptionGroup.__new__() argument 1 must be str, not {}",
                    type_name(_py, message_obj)
                );
                let _ = raise_exception::<u64>(_py, "TypeError", &msg);
                dec_ref_bits(_py, args_bits);
                return std::ptr::null_mut();
            }
        } else {
            let msg = format!(
                "BaseExceptionGroup.__new__() argument 1 must be str, not {}",
                type_name(_py, message_obj)
            );
            let _ = raise_exception::<u64>(_py, "TypeError", &msg);
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        }
        let Some(collected) = exception_group_collect_exceptions(_py, exceptions_bits) else {
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        };
        let builtins = builtin_classes(_py);
        let strict_exception = issubclass_bits(class_bits, builtins.exception);
        if strict_exception && !collected.all_exception {
            let _ = raise_exception::<u64>(
                _py,
                "TypeError",
                "Cannot nest BaseExceptions in an ExceptionGroup",
            );
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        }
        let Some(bits) = exception_group_alloc(
            _py,
            class_bits,
            message_bits,
            exceptions_bits,
            &collected.items,
            None,
        ) else {
            dec_ref_bits(_py, args_bits);
            return std::ptr::null_mut();
        };
        dec_ref_bits(_py, args_bits);
        obj_from_bits(bits).as_ptr().unwrap_or(std::ptr::null_mut())
    }
}

pub extern "C" fn molt_exceptiongroup_init(self_bits: u64, args_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "exception init expects exception instance",
            );
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "exception init expects exception instance",
                );
            }
        }
        let norm_bits = exception_normalize_args(_py, args_bits);
        if obj_from_bits(norm_bits).is_none() {
            if !obj_from_bits(args_bits).is_none() {
                dec_ref_bits(_py, args_bits);
            }
            return MoltObject::none().bits();
        }
        unsafe {
            inc_ref_bits(_py, norm_bits);
            let args_slot = self_ptr.add(8 * std::mem::size_of::<u64>()) as *mut u64;
            let old_bits = *args_slot;
            if old_bits != norm_bits {
                dec_ref_bits(_py, old_bits);
                *args_slot = norm_bits;
            }
        }
        dec_ref_bits(_py, norm_bits);
        if !obj_from_bits(args_bits).is_none() {
            dec_ref_bits(_py, args_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exceptiongroup_subgroup(self_bits: u64, matcher_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
        }
        let mut class_bits = unsafe { exception_class_bits(self_ptr) };
        if obj_from_bits(class_bits).is_none() || class_bits == 0 {
            class_bits = unsafe { exception_type_bits(_py, exception_kind_bits(self_ptr)) };
        }
        let base_group_bits = builtin_classes(_py).base_exception_group;
        if base_group_bits == 0 || !issubclass_bits(class_bits, base_group_bits) {
            let type_label = type_name(_py, self_obj);
            let msg = format!(
                "descriptor 'subgroup' for 'BaseExceptionGroup' objects doesn't apply to a '{type_label}' object"
            );
            return raise_exception::<u64>(_py, "TypeError", &msg);
        }
        let Some(matcher) = exception_group_parse_matcher(_py, matcher_bits) else {
            return MoltObject::none().bits();
        };
        if let Some(matches) = exception_group_matcher_matches(_py, &matcher, self_bits) {
            if matches {
                inc_ref_bits(_py, self_bits);
                return self_bits;
            }
        } else {
            return MoltObject::none().bits();
        }
        let Some((match_item, _rest_item)) = exception_group_split_node(_py, self_bits, &matcher)
        else {
            return MoltObject::none().bits();
        };
        if let Some(item) = match_item {
            if !item.owned {
                inc_ref_bits(_py, item.bits);
            }
            return item.bits;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exceptiongroup_split(self_bits: u64, matcher_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
        }
        let mut class_bits = unsafe { exception_class_bits(self_ptr) };
        if obj_from_bits(class_bits).is_none() || class_bits == 0 {
            class_bits = unsafe { exception_type_bits(_py, exception_kind_bits(self_ptr)) };
        }
        let base_group_bits = builtin_classes(_py).base_exception_group;
        if base_group_bits == 0 || !issubclass_bits(class_bits, base_group_bits) {
            let type_label = type_name(_py, self_obj);
            let msg = format!(
                "descriptor 'split' for 'BaseExceptionGroup' objects doesn't apply to a '{type_label}' object"
            );
            return raise_exception::<u64>(_py, "TypeError", &msg);
        }
        let Some(matcher) = exception_group_parse_matcher(_py, matcher_bits) else {
            return MoltObject::none().bits();
        };
        if let Some(matches) = exception_group_matcher_matches(_py, &matcher, self_bits) {
            if matches {
                return exception_group_make_pair_tuple(
                    _py,
                    Some(ExceptionGroupItem {
                        bits: self_bits,
                        owned: false,
                    }),
                    None,
                );
            }
        } else {
            return MoltObject::none().bits();
        }
        let Some((match_item, rest_item)) = exception_group_split_node(_py, self_bits, &matcher)
        else {
            return MoltObject::none().bits();
        };
        exception_group_make_pair_tuple(_py, match_item, rest_item)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exceptiongroup_derive(self_bits: u64, exceptions_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let self_obj = obj_from_bits(self_bits);
        let Some(self_ptr) = self_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(self_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
        }
        let mut class_bits = unsafe { exception_class_bits(self_ptr) };
        if obj_from_bits(class_bits).is_none() || class_bits == 0 {
            class_bits = unsafe { exception_type_bits(_py, exception_kind_bits(self_ptr)) };
        }
        let base_group_bits = builtin_classes(_py).base_exception_group;
        if base_group_bits == 0 || !issubclass_bits(class_bits, base_group_bits) {
            let type_label = type_name(_py, self_obj);
            let msg = format!(
                "descriptor 'derive' for 'BaseExceptionGroup' objects doesn't apply to a '{type_label}' object"
            );
            return raise_exception::<u64>(_py, "TypeError", &msg);
        }
        let Some(collected) = exception_group_collect_exceptions(_py, exceptions_bits) else {
            return MoltObject::none().bits();
        };
        let builtins = builtin_classes(_py);
        let mut target_class = class_bits;
        if issubclass_bits(class_bits, builtins.exception) && !collected.all_exception {
            target_class = builtins.base_exception_group;
        }
        let message_bits = exception_group_message_bits(_py, self_ptr);
        exception_group_alloc(
            _py,
            target_class,
            message_bits,
            exceptions_bits,
            &collected.items,
            None,
        )
        .unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exceptiongroup_match(exc_bits: u64, matcher_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let none_bits = MoltObject::none().bits();
        if obj_from_bits(exc_bits).is_none() {
            return exception_group_make_pair_tuple(_py, None, None);
        }
        let Some(match_bits) = exception_group_parse_except_star_matcher(_py, matcher_bits) else {
            return MoltObject::none().bits();
        };
        let exc_obj = obj_from_bits(exc_bits);
        let Some(exc_ptr) = exc_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            if object_type_id(exc_ptr) != TYPE_ID_EXCEPTION {
                return raise_exception::<u64>(_py, "TypeError", "expected exception object");
            }
        }
        let mut exc_class_bits = unsafe { exception_class_bits(exc_ptr) };
        if obj_from_bits(exc_class_bits).is_none() || exc_class_bits == 0 {
            exc_class_bits = unsafe { exception_type_bits(_py, exception_kind_bits(exc_ptr)) };
        }
        let base_group_bits = builtin_classes(_py).base_exception_group;
        if issubclass_bits(exc_class_bits, base_group_bits) {
            let is_match = isinstance_bits(_py, exc_bits, match_bits);
            if is_match {
                return exception_group_make_pair_tuple(
                    _py,
                    Some(ExceptionGroupItem {
                        bits: exc_bits,
                        owned: false,
                    }),
                    None,
                );
            }
            let matcher = ExceptionGroupMatcher::Type(match_bits);
            let Some((match_item, rest_item)) = exception_group_split_node(_py, exc_bits, &matcher)
            else {
                return MoltObject::none().bits();
            };
            return exception_group_make_pair_tuple(_py, match_item, rest_item);
        }
        if !isinstance_bits(_py, exc_bits, match_bits) {
            return exception_group_make_pair_tuple(
                _py,
                None,
                Some(ExceptionGroupItem {
                    bits: exc_bits,
                    owned: false,
                }),
            );
        }
        let exc_type_bits = type_of_bits(_py, exc_bits);
        let builtins = builtin_classes(_py);
        let group_class_bits = if issubclass_bits(exc_type_bits, builtins.exception) {
            builtins.exception_group
        } else {
            builtins.base_exception_group
        };
        let tuple_ptr = alloc_tuple(_py, &[exc_bits]);
        if tuple_ptr.is_null() {
            return none_bits;
        }
        let tuple_bits = MoltObject::from_ptr(tuple_ptr).bits();
        let msg_ptr = alloc_string(_py, b"");
        if msg_ptr.is_null() {
            dec_ref_bits(_py, tuple_bits);
            return none_bits;
        }
        let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
        let group_bits = exception_group_alloc(
            _py,
            group_class_bits,
            msg_bits,
            tuple_bits,
            &[exc_bits],
            Some(tuple_bits),
        );
        dec_ref_bits(_py, msg_bits);
        let Some(bits) = group_bits else {
            return none_bits;
        };
        if let Some(group_ptr) = obj_from_bits(bits).as_ptr() {
            unsafe {
                exception_group_copy_metadata(_py, group_ptr, exc_ptr, false, true, false, false);
            }
        }
        exception_group_make_pair_tuple(_py, Some(ExceptionGroupItem { bits, owned: true }), None)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_exceptiongroup_combine(list_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "expected exception object");
        };
        unsafe {
            let type_id = object_type_id(list_ptr);
            if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "second argument (exceptions) must be a sequence",
                );
            }
            let elems = seq_vec_ref(list_ptr);
            if elems.is_empty() {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "second argument (exceptions) must be a non-empty sequence",
                );
            }
            let builtins = builtin_classes(_py);
            let mut all_exception = true;
            for (idx, &item_bits) in elems.iter().enumerate() {
                let item_class = type_of_bits(_py, item_bits);
                if !issubclass_bits(item_class, builtins.base_exception) {
                    let msg =
                        format!("Item {idx} of second argument (exceptions) is not an exception");
                    return raise_exception::<u64>(_py, "ValueError", &msg);
                }
                if !issubclass_bits(item_class, builtins.exception) {
                    all_exception = false;
                }
            }
            let group_class = if all_exception {
                builtins.exception_group
            } else {
                builtins.base_exception_group
            };
            let msg_ptr = alloc_string(_py, b"");
            if msg_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let msg_bits = MoltObject::from_ptr(msg_ptr).bits();
            let out = exception_group_alloc(_py, group_class, msg_bits, list_bits, elems, None)
                .unwrap_or_else(|| MoltObject::none().bits());
            dec_ref_bits(_py, msg_bits);
            out
        }
    })
}
