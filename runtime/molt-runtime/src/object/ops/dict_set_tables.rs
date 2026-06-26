use super::*;

pub(crate) extern "C" fn dict_keys_method(self_bits: u64) -> i64 {
    molt_dict_keys(self_bits) as i64
}

pub(crate) extern "C" fn dict_values_method(self_bits: u64) -> i64 {
    molt_dict_values(self_bits) as i64
}

pub(crate) extern "C" fn dict_items_method(self_bits: u64) -> i64 {
    molt_dict_items(self_bits) as i64
}

pub(crate) extern "C" fn dict_get_method(self_bits: u64, key_bits: u64, default_bits: u64) -> i64 {
    molt_dict_get(self_bits, key_bits, default_bits) as i64
}

pub(crate) extern "C" fn dict_clear_method(self_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(self_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.clear expects dict");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.clear expects dict");
            }
            dict_clear_in_place(_py, ptr);
        }
        MoltObject::none().bits() as i64
    })
}

pub(crate) extern "C" fn dict_copy_method(self_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(self_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.copy expects dict");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.copy expects dict");
            }
            let pairs = dict_order(ptr).clone();
            let out_ptr = alloc_dict_with_pairs(_py, pairs.as_slice());
            if out_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            MoltObject::from_ptr(out_ptr).bits() as i64
        }
    })
}

pub(crate) extern "C" fn dict_popitem_method(self_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(self_bits);
        let Some(ptr) = obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict.popitem expects dict");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict.popitem expects dict");
            }
            let order = dict_order(ptr);
            if order.len() < 2 {
                return raise_exception::<_>(_py, "KeyError", "popitem(): dictionary is empty");
            }
            let key_bits = order[order.len() - 2];
            let val_bits = order[order.len() - 1];
            let item_ptr = alloc_tuple(_py, &[key_bits, val_bits]);
            if item_ptr.is_null() {
                return MoltObject::none().bits() as i64;
            }
            order.truncate(order.len() - 2);
            let hashes = dict_hashes(ptr);
            hashes.truncate(hashes.len().saturating_sub(1));
            let entries = order.len() / 2;
            let table = dict_table(ptr);
            let capacity = dict_table_capacity(entries.max(1));
            dict_rebuild(_py, order, hashes, table, capacity);
            if order.is_empty() {
                (*header_from_obj_ptr(ptr)).flags &= !crate::object::HEADER_FLAG_CONTAINS_REFS;
            }
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, val_bits);
            MoltObject::from_ptr(item_ptr).bits() as i64
        }
    })
}

pub(crate) extern "C" fn dict_setdefault_method(
    self_bits: u64,
    key_bits: u64,
    default_bits: u64,
) -> i64 {
    molt_dict_setdefault(self_bits, key_bits, default_bits) as i64
}

pub(crate) extern "C" fn dict_fromkeys_method(
    self_bits: u64,
    iterable_bits: u64,
    default_bits: u64,
) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        let class_bits = if let Some(ptr) = maybe_ptr_from_bits(self_bits) {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_TYPE {
                    self_bits
                } else {
                    type_of_bits(_py, self_bits)
                }
            }
        } else {
            type_of_bits(_py, self_bits)
        };
        let builtins = builtin_classes(_py);
        if !issubclass_bits(class_bits, builtins.dict) {
            return raise_exception::<_>(_py, "TypeError", "dict.fromkeys expects dict type");
        }
        let capacity_hint = {
            let obj = obj_from_bits(iterable_bits);
            let mut hint = if let Some(ptr) = obj.as_ptr() {
                unsafe {
                    match object_type_id(ptr) {
                        TYPE_ID_LIST => list_len(ptr),
                        TYPE_ID_TUPLE => tuple_len(ptr),
                        TYPE_ID_DICT => dict_len(ptr),
                        TYPE_ID_SET | TYPE_ID_FROZENSET => set_len(ptr),
                        TYPE_ID_DICT_KEYS_VIEW
                        | TYPE_ID_DICT_VALUES_VIEW
                        | TYPE_ID_DICT_ITEMS_VIEW => dict_view_len(ptr),
                        TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => bytes_len(ptr),
                        TYPE_ID_STRING => string_len(ptr),
                        TYPE_ID_INTARRAY => intarray_len(ptr),
                        TYPE_ID_RANGE => {
                            if let Some((start, stop, step)) = range_components_bigint(ptr) {
                                let len = range_len_bigint(&start, &stop, &step);
                                len.to_usize().unwrap_or(usize::MAX)
                            } else {
                                0
                            }
                        }
                        _ => 0,
                    }
                }
            } else {
                0
            };
            let max_entries = (isize::MAX as usize) / 2;
            if hint > max_entries {
                hint = max_entries;
            }
            hint
        };
        let dict_bits = if class_bits == builtins.dict {
            molt_dict_new(capacity_hint as u64)
        } else {
            let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
                return MoltObject::none().bits() as i64;
            };
            unsafe { call_class_init_with_args(_py, class_ptr, &[]) }
        };
        if exception_pending(_py) {
            return MoltObject::none().bits() as i64;
        }
        let iter_bits = molt_iter(iterable_bits);
        if obj_from_bits(iter_bits).is_none() {
            return raise_not_iterable(_py, iterable_bits);
        }
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits() as i64;
            }
            let pair_obj = obj_from_bits(pair_bits);
            let Some(pair_ptr) = pair_obj.as_ptr() else {
                return MoltObject::none().bits() as i64;
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return raise_exception::<_>(_py, "TypeError", "object is not an iterator");
                }
                let done_bits = elems[1];
                if is_truthy(_py, obj_from_bits(done_bits)) {
                    break;
                }
                let key_bits = elems[0];
                let _ = molt_store_index(dict_bits, key_bits, default_bits);
                if exception_pending(_py) {
                    return MoltObject::none().bits() as i64;
                }
            }
        }
        dict_bits as i64
    })
}

pub(crate) extern "C" fn dict_update_method(self_bits: u64, other_bits: u64) -> i64 {
    crate::with_gil_entry_nopanic!(_py, {
        if other_bits == missing_bits(_py) {
            return MoltObject::none().bits() as i64;
        }
        molt_dict_update(self_bits, other_bits) as i64
    })
}

pub(crate) unsafe fn dict_update_set_via_store(
    _py: &PyToken<'_>,
    target_bits: u64,
    key_bits: u64,
    val_bits: u64,
) {
    crate::gil_assert();
    let _ = molt_store_index(target_bits, key_bits, val_bits);
}

pub(crate) unsafe fn dict_inc_in_place(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key_bits: u64,
    delta_bits: u64,
) -> bool {
    unsafe {
        if !ensure_hashable(_py, key_bits, HashContext::DictKey) {
            return false;
        }
        let current_bits =
            dict_get_in_place(_py, dict_ptr, key_bits).unwrap_or(MoltObject::from_int(0).bits());
        if exception_pending(_py) {
            return false;
        }

        if let (Some(current), Some(delta)) = (
            obj_from_bits(current_bits).as_int(),
            obj_from_bits(delta_bits).as_int(),
        ) && let Some(sum) = current.checked_add(delta)
        {
            let sum_bits = MoltObject::from_int(sum).bits();
            dict_set_in_place(_py, dict_ptr, key_bits, sum_bits);
            return !exception_pending(_py);
        }

        let sum_bits = molt_add(current_bits, delta_bits);
        if obj_from_bits(sum_bits).is_none() {
            return false;
        }
        dict_set_in_place(_py, dict_ptr, key_bits, sum_bits);
        dec_ref_bits(_py, sum_bits);
        !exception_pending(_py)
    }
}

fn bits_as_int(bits: u64) -> Option<i64> {
    obj_from_bits(bits).as_int()
}

pub(crate) unsafe fn dict_inc_prehashed_string_key_in_place(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key_bits: u64,
    delta_bits: u64,
) -> Option<bool> {
    unsafe {
        let key_obj = obj_from_bits(key_bits);
        let key_ptr = key_obj.as_ptr()?;
        if object_type_id(key_ptr) != TYPE_ID_STRING {
            return None;
        }
        let delta = bits_as_int(delta_bits)?;
        let key_bytes = std::slice::from_raw_parts(string_bytes(key_ptr), string_len(key_ptr));
        let hash = hash_string_bytes(_py, key_bytes) as u64;

        let order = dict_order(dict_ptr);
        let hashes = dict_hashes(dict_ptr);
        let table = dict_table(dict_ptr);
        if !table.is_empty() {
            let mask = table.len() - 1;
            let mut slot = (hash as usize) & mask;
            loop {
                let entry = table[slot];
                if entry == 0 {
                    break;
                }
                let entry_idx = entry - 1;
                if entry_idx * 2 >= order.len() {
                    slot = (slot + 1) & mask;
                    continue;
                }
                if hashes.get(entry_idx).copied() != Some(hash) {
                    slot = (slot + 1) & mask;
                    continue;
                }
                let entry_key_bits = order[entry_idx * 2];
                let mut keys_match = entry_key_bits == key_bits;
                if !keys_match {
                    let Some(entry_key_ptr) = obj_from_bits(entry_key_bits).as_ptr() else {
                        // continue probing
                        slot = (slot + 1) & mask;
                        continue;
                    };
                    if object_type_id(entry_key_ptr) == TYPE_ID_STRING {
                        let entry_len = string_len(entry_key_ptr);
                        if entry_len == key_bytes.len() {
                            let entry_bytes =
                                std::slice::from_raw_parts(string_bytes(entry_key_ptr), entry_len);
                            keys_match = entry_bytes == key_bytes;
                        }
                    }
                }
                if keys_match {
                    profile_hit_unchecked(&DICT_STR_INT_PREHASH_HIT_COUNT);
                    let val_idx = entry_idx * 2 + 1;
                    let current_bits = order[val_idx];
                    let sum_bits: u64;
                    let mut sum_owned = false;
                    if let Some(current) = obj_from_bits(current_bits).as_int() {
                        if let Some(sum) = current.checked_add(delta) {
                            sum_bits = MoltObject::from_int(sum).bits();
                        } else {
                            sum_bits = molt_add(current_bits, delta_bits);
                            if obj_from_bits(sum_bits).is_none() {
                                return Some(false);
                            }
                            sum_owned = true;
                        }
                    } else {
                        sum_bits = molt_add(current_bits, delta_bits);
                        if obj_from_bits(sum_bits).is_none() {
                            return Some(false);
                        }
                        sum_owned = true;
                    }
                    if current_bits != sum_bits {
                        dec_ref_bits(_py, current_bits);
                        inc_ref_bits(_py, sum_bits);
                        order[val_idx] = sum_bits;
                    }
                    if sum_owned {
                        dec_ref_bits(_py, sum_bits);
                    }
                    return Some(!exception_pending(_py));
                }
                slot = (slot + 1) & mask;
            }
        }

        let sum_bits = MoltObject::from_int(delta).bits();
        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, hashes, table, capacity);
            if exception_pending(_py) {
                return Some(false);
            }
        }
        if !reserve_dict_order(_py, order, 2)
            || !reserve_hashes(_py, hashes, 1, "dict allocation failed")
        {
            return Some(false);
        }
        order.push(key_bits);
        order.push(sum_bits);
        hashes.push(hash);
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, sum_bits);
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        profile_hit_unchecked(&DICT_STR_INT_PREHASH_MISS_COUNT);
        Some(!exception_pending(_py))
    }
}

unsafe fn dict_inc_with_string_token_fallback(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    token: &[u8],
    delta_bits: u64,
    last_bits: &mut u64,
    had_any: &mut bool,
) -> bool {
    unsafe {
        let key_ptr = alloc_string(_py, token);
        if key_ptr.is_null() {
            return false;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        if let Some(done) =
            dict_inc_prehashed_string_key_in_place(_py, dict_ptr, key_bits, delta_bits)
        {
            if !done {
                dec_ref_bits(_py, key_bits);
                return false;
            }
        } else if !dict_inc_in_place(_py, dict_ptr, key_bits, delta_bits) {
            dec_ref_bits(_py, key_bits);
            return false;
        }
        if *had_any && !obj_from_bits(*last_bits).is_none() {
            dec_ref_bits(_py, *last_bits);
        }
        inc_ref_bits(_py, key_bits);
        *last_bits = key_bits;
        *had_any = true;
        dec_ref_bits(_py, key_bits);
        true
    }
}

unsafe fn dict_inc_with_string_token(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    token: &[u8],
    delta_bits: u64,
    last_bits: &mut u64,
    had_any: &mut bool,
) -> bool {
    unsafe {
        let hash = hash_string_bytes(_py, token) as u64;
        {
            let order = dict_order(dict_ptr);
            let hashes = dict_hashes(dict_ptr);
            let table = dict_table(dict_ptr);
            if !table.is_empty() {
                let mask = table.len() - 1;
                let mut slot = (hash as usize) & mask;
                loop {
                    let entry = table[slot];
                    if entry == 0 {
                        break;
                    }
                    let entry_idx = entry - 1;
                    if entry_idx * 2 >= order.len() {
                        slot = (slot + 1) & mask;
                        continue;
                    }
                    if hashes.get(entry_idx).copied() != Some(hash) {
                        slot = (slot + 1) & mask;
                        continue;
                    }
                    let entry_key_bits = order[entry_idx * 2];
                    let Some(entry_key_ptr) = obj_from_bits(entry_key_bits).as_ptr() else {
                        return dict_inc_with_string_token_fallback(
                            _py, dict_ptr, token, delta_bits, last_bits, had_any,
                        );
                    };
                    if object_type_id(entry_key_ptr) != TYPE_ID_STRING {
                        return dict_inc_with_string_token_fallback(
                            _py, dict_ptr, token, delta_bits, last_bits, had_any,
                        );
                    }
                    let entry_len = string_len(entry_key_ptr);
                    if entry_len == token.len() {
                        let entry_bytes =
                            std::slice::from_raw_parts(string_bytes(entry_key_ptr), entry_len);
                        if entry_bytes == token {
                            let val_idx = entry_idx * 2 + 1;
                            let current_bits = order[val_idx];
                            let sum_bits: u64;
                            let mut sum_owned = false;
                            if let (Some(current), Some(delta)) = (
                                obj_from_bits(current_bits).as_int(),
                                obj_from_bits(delta_bits).as_int(),
                            ) {
                                if let Some(sum) = current.checked_add(delta) {
                                    sum_bits = MoltObject::from_int(sum).bits();
                                } else {
                                    sum_bits = molt_add(current_bits, delta_bits);
                                    if obj_from_bits(sum_bits).is_none() {
                                        return false;
                                    }
                                    sum_owned = true;
                                }
                            } else {
                                sum_bits = molt_add(current_bits, delta_bits);
                                if obj_from_bits(sum_bits).is_none() {
                                    return false;
                                }
                                sum_owned = true;
                            }
                            if current_bits != sum_bits {
                                dec_ref_bits(_py, current_bits);
                                inc_ref_bits(_py, sum_bits);
                                order[val_idx] = sum_bits;
                            }
                            if sum_owned {
                                dec_ref_bits(_py, sum_bits);
                            }
                            if *had_any && !obj_from_bits(*last_bits).is_none() {
                                dec_ref_bits(_py, *last_bits);
                            }
                            inc_ref_bits(_py, entry_key_bits);
                            *last_bits = entry_key_bits;
                            *had_any = true;
                            return true;
                        }
                    }
                    slot = (slot + 1) & mask;
                }
            }
        }

        let key_ptr = alloc_string(_py, token);
        if key_ptr.is_null() {
            return false;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let zero_bits = MoltObject::from_int(0).bits();
        let sum_bits: u64;
        let mut sum_owned = false;
        if let Some(delta) = obj_from_bits(delta_bits).as_int() {
            sum_bits = MoltObject::from_int(delta).bits();
        } else {
            sum_bits = molt_add(zero_bits, delta_bits);
            if obj_from_bits(sum_bits).is_none() {
                dec_ref_bits(_py, key_bits);
                return false;
            }
            sum_owned = true;
        }
        let order = dict_order(dict_ptr);
        let hashes = dict_hashes(dict_ptr);
        let table = dict_table(dict_ptr);
        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, hashes, table, capacity);
            if exception_pending(_py) {
                if sum_owned {
                    dec_ref_bits(_py, sum_bits);
                }
                dec_ref_bits(_py, key_bits);
                return false;
            }
        }
        if !reserve_dict_order(_py, order, 2)
            || !reserve_hashes(_py, hashes, 1, "dict allocation failed")
        {
            if sum_owned {
                dec_ref_bits(_py, sum_bits);
            }
            dec_ref_bits(_py, key_bits);
            return false;
        }
        order.push(key_bits);
        order.push(sum_bits);
        hashes.push(hash);
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, sum_bits);
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        if sum_owned {
            dec_ref_bits(_py, sum_bits);
        }
        if *had_any && !obj_from_bits(*last_bits).is_none() {
            dec_ref_bits(_py, *last_bits);
        }
        inc_ref_bits(_py, key_bits);
        *last_bits = key_bits;
        *had_any = true;
        dec_ref_bits(_py, key_bits);
        true
    }
}

unsafe fn dict_setdefault_empty_list_with_string_token(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    token: &[u8],
) -> Option<u64> {
    unsafe {
        let hash = hash_string_bytes(_py, token) as u64;
        {
            let order = dict_order(dict_ptr);
            let hashes = dict_hashes(dict_ptr);
            let table = dict_table(dict_ptr);
            if !table.is_empty() {
                let mask = table.len() - 1;
                let mut slot = (hash as usize) & mask;
                loop {
                    let entry = table[slot];
                    if entry == 0 {
                        break;
                    }
                    let entry_idx = entry - 1;
                    if entry_idx * 2 >= order.len() {
                        slot = (slot + 1) & mask;
                        continue;
                    }
                    if hashes.get(entry_idx).copied() != Some(hash) {
                        slot = (slot + 1) & mask;
                        continue;
                    }
                    let entry_key_bits = order[entry_idx * 2];
                    let Some(entry_key_ptr) = obj_from_bits(entry_key_bits).as_ptr() else {
                        slot = (slot + 1) & mask;
                        continue;
                    };
                    if object_type_id(entry_key_ptr) == TYPE_ID_STRING {
                        let entry_len = string_len(entry_key_ptr);
                        if entry_len == token.len() {
                            let entry_bytes =
                                std::slice::from_raw_parts(string_bytes(entry_key_ptr), entry_len);
                            if entry_bytes == token {
                                let val_bits = order[entry_idx * 2 + 1];
                                inc_ref_bits(_py, val_bits);
                                return Some(val_bits);
                            }
                        }
                    }
                    slot = (slot + 1) & mask;
                }
            }
        }

        let key_ptr = alloc_string(_py, token);
        if key_ptr.is_null() {
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let default_ptr = alloc_list(_py, &[]);
        if default_ptr.is_null() {
            dec_ref_bits(_py, key_bits);
            return None;
        }
        let default_bits = MoltObject::from_ptr(default_ptr).bits();
        let order = dict_order(dict_ptr);
        let hashes = dict_hashes(dict_ptr);
        let table = dict_table(dict_ptr);
        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, hashes, table, capacity);
            if exception_pending(_py) {
                dec_ref_bits(_py, default_bits);
                dec_ref_bits(_py, key_bits);
                return None;
            }
        }
        if !reserve_dict_order(_py, order, 2)
            || !reserve_hashes(_py, hashes, 1, "dict allocation failed")
        {
            dec_ref_bits(_py, default_bits);
            dec_ref_bits(_py, key_bits);
            return None;
        }
        order.push(key_bits);
        order.push(default_bits);
        hashes.push(hash);
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, default_bits);
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        if exception_pending(_py) {
            dec_ref_bits(_py, default_bits);
            dec_ref_bits(_py, key_bits);
            return None;
        }
        inc_ref_bits(_py, default_bits);
        dec_ref_bits(_py, default_bits);
        dec_ref_bits(_py, key_bits);
        Some(default_bits)
    }
}

unsafe fn split_dict_inc_result_tuple(_py: &PyToken<'_>, last_bits: u64, had_any: bool) -> u64 {
    let had_any_bits = MoltObject::from_bool(had_any).bits();
    let pair_ptr = alloc_tuple(_py, &[last_bits, had_any_bits]);
    if pair_ptr.is_null() {
        if had_any && !obj_from_bits(last_bits).is_none() {
            dec_ref_bits(_py, last_bits);
        }
        return MoltObject::none().bits();
    }
    if had_any && !obj_from_bits(last_bits).is_none() {
        dec_ref_bits(_py, last_bits);
    }
    MoltObject::from_ptr(pair_ptr).bits()
}

fn parse_ascii_i64_field(_py: &PyToken<'_>, field: &[u8]) -> Option<i64> {
    let mut start = 0usize;
    let mut end = field.len();
    while start < end && field[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && field[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    let trimmed = &field[start..end];
    if trimmed.is_empty() {
        profile_hit_unchecked(&ASCII_I64_PARSE_FAIL_COUNT);
        raise_exception::<()>(
            _py,
            "ValueError",
            "invalid literal for int() with base 10: ''",
        );
        return None;
    }
    let mut idx = 0usize;
    let mut neg = false;
    if trimmed[0] == b'+' {
        idx = 1;
    } else if trimmed[0] == b'-' {
        neg = true;
        idx = 1;
    }
    if idx >= trimmed.len() {
        profile_hit_unchecked(&ASCII_I64_PARSE_FAIL_COUNT);
        let shown = String::from_utf8_lossy(trimmed);
        let msg = format!("invalid literal for int() with base 10: '{shown}'");
        raise_exception::<()>(_py, "ValueError", &msg);
        return None;
    }
    let mut value: i128 = 0;
    while idx < trimmed.len() {
        let b = trimmed[idx];
        if !b.is_ascii_digit() {
            profile_hit_unchecked(&ASCII_I64_PARSE_FAIL_COUNT);
            let shown = String::from_utf8_lossy(trimmed);
            let msg = format!("invalid literal for int() with base 10: '{shown}'");
            raise_exception::<()>(_py, "ValueError", &msg);
            return None;
        }
        value = value * 10 + i128::from((b - b'0') as i64);
        idx += 1;
    }
    if neg {
        value = -value;
    }
    if value < i128::from(i64::MIN) || value > i128::from(i64::MAX) {
        profile_hit_unchecked(&ASCII_I64_PARSE_FAIL_COUNT);
        let shown = String::from_utf8_lossy(trimmed);
        let msg = format!("invalid literal for int() with base 10: '{shown}'");
        raise_exception::<()>(_py, "ValueError", &msg);
        None
    } else {
        Some(value as i64)
    }
}

#[inline]
fn is_ascii_split_whitespace_byte(byte: u8) -> bool {
    matches!(byte, b' ' | b'\n' | b'\r' | b'\t' | 0x0b | 0x0c)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse2")]
unsafe fn find_ascii_split_whitespace_sse2(bytes: &[u8], start: usize) -> usize {
    unsafe {
        use std::arch::x86_64::*;
        let mut i = start;
        let len = bytes.len();
        let sp = _mm_set1_epi8(b' ' as i8);
        let nl = _mm_set1_epi8(b'\n' as i8);
        let cr = _mm_set1_epi8(b'\r' as i8);
        let tab = _mm_set1_epi8(b'\t' as i8);
        let vt = _mm_set1_epi8(0x0b_i8);
        let ff = _mm_set1_epi8(0x0c_i8);
        while i + 16 <= len {
            let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
            let mut mask_vec = _mm_or_si128(_mm_cmpeq_epi8(chunk, sp), _mm_cmpeq_epi8(chunk, nl));
            mask_vec = _mm_or_si128(mask_vec, _mm_cmpeq_epi8(chunk, cr));
            mask_vec = _mm_or_si128(mask_vec, _mm_cmpeq_epi8(chunk, tab));
            mask_vec = _mm_or_si128(mask_vec, _mm_cmpeq_epi8(chunk, vt));
            mask_vec = _mm_or_si128(mask_vec, _mm_cmpeq_epi8(chunk, ff));
            let mask = _mm_movemask_epi8(mask_vec) as u32;
            if mask != 0 {
                return i + mask.trailing_zeros() as usize;
            }
            i += 16;
        }
        while i < len {
            if is_ascii_split_whitespace_byte(bytes[i]) {
                return i;
            }
            i += 1;
        }
        len
    }
}

/// NEON variant: find first ASCII whitespace byte in slice starting at `start`.
#[cfg(target_arch = "aarch64")]
unsafe fn find_ascii_split_whitespace_neon(bytes: &[u8], start: usize) -> usize {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = start;
        let len = bytes.len();
        let sp = vdupq_n_u8(b' ');
        let nl = vdupq_n_u8(b'\n');
        let cr = vdupq_n_u8(b'\r');
        let tab = vdupq_n_u8(b'\t');
        let vt = vdupq_n_u8(0x0b);
        let ff = vdupq_n_u8(0x0c);
        while i + 16 <= len {
            let chunk = vld1q_u8(bytes.as_ptr().add(i));
            let is_ws = vorrq_u8(
                vorrq_u8(
                    vorrq_u8(vceqq_u8(chunk, sp), vceqq_u8(chunk, nl)),
                    vceqq_u8(chunk, cr),
                ),
                vorrq_u8(
                    vceqq_u8(chunk, tab),
                    vorrq_u8(vceqq_u8(chunk, vt), vceqq_u8(chunk, ff)),
                ),
            );
            if vmaxvq_u8(is_ws) != 0 {
                // Found whitespace in this chunk — scan for exact position
                let mut buf = [0u8; 16];
                vst1q_u8(buf.as_mut_ptr(), is_ws);
                for (j, &byte) in buf.iter().enumerate() {
                    if byte != 0 {
                        return i + j;
                    }
                }
            }
            i += 16;
        }
        while i < len {
            if is_ascii_split_whitespace_byte(bytes[i]) {
                return i;
            }
            i += 1;
        }
        len
    }
}

#[cfg(target_arch = "wasm32")]
unsafe fn find_ascii_split_whitespace_wasm32(bytes: &[u8], start: usize) -> usize {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = start;
        let len = bytes.len();
        let sp = u8x16_splat(b' ');
        let nl = u8x16_splat(b'\n');
        let cr = u8x16_splat(b'\r');
        let tab = u8x16_splat(b'\t');
        let vt = u8x16_splat(0x0b);
        let ff = u8x16_splat(0x0c);
        while i + 16 <= len {
            let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
            let is_ws = v128_or(
                v128_or(
                    v128_or(u8x16_eq(chunk, sp), u8x16_eq(chunk, nl)),
                    u8x16_eq(chunk, cr),
                ),
                v128_or(
                    u8x16_eq(chunk, tab),
                    v128_or(u8x16_eq(chunk, vt), u8x16_eq(chunk, ff)),
                ),
            );
            let mask = u8x16_bitmask(is_ws);
            if mask != 0 {
                return i + mask.trailing_zeros() as usize;
            }
            i += 16;
        }
        while i < len {
            if is_ascii_split_whitespace_byte(bytes[i]) {
                return i;
            }
            i += 1;
        }
        len
    }
}

fn find_ascii_split_whitespace(bytes: &[u8], start: usize) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { find_ascii_split_whitespace_sse2(bytes, start) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { find_ascii_split_whitespace_neon(bytes, start) };
    }
    #[cfg(target_arch = "wasm32")]
    {
        return unsafe { find_ascii_split_whitespace_wasm32(bytes, start) };
    }
    #[allow(unreachable_code)]
    {
        let mut i = start;
        while i < bytes.len() {
            if is_ascii_split_whitespace_byte(bytes[i]) {
                return i;
            }
            i += 1;
        }
        bytes.len()
    }
}

unsafe fn split_ascii_whitespace_dict_inc_tokens(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    line_bytes: &[u8],
    delta_bits: u64,
    last_bits: &mut u64,
    had_any: &mut bool,
) -> bool {
    unsafe {
        let mut idx = 0usize;
        let len = line_bytes.len();
        while idx < len {
            while idx < len && is_ascii_split_whitespace_byte(line_bytes[idx]) {
                idx += 1;
            }
            if idx >= len {
                break;
            }
            let token_start = idx;
            let token_end = find_ascii_split_whitespace(line_bytes, token_start);
            if !dict_inc_with_string_token(
                _py,
                dict_ptr,
                &line_bytes[token_start..token_end],
                delta_bits,
                last_bits,
                had_any,
            ) {
                return false;
            }
            idx = token_end;
        }
        true
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_ws_dict_inc(
    line_bits: u64,
    dict_bits: u64,
    delta_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let line_obj = obj_from_bits(line_bits);
        let dict_obj = obj_from_bits(dict_bits);
        let Some(line_ptr) = line_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "split expects str");
        };
        let Some(dict_ptr_raw) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
        };
        unsafe {
            if object_type_id(line_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "split expects str");
            }
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, dict_ptr_raw) else {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            }
            let line_bytes =
                std::slice::from_raw_parts(string_bytes(line_ptr), string_len(line_ptr));
            let mut last_bits = MoltObject::none().bits();
            let mut had_any = false;
            if line_bytes.is_ascii() {
                profile_hit_unchecked(&SPLIT_WS_ASCII_FAST_PATH_COUNT);
                if !split_ascii_whitespace_dict_inc_tokens(
                    _py,
                    dict_ptr,
                    line_bytes,
                    delta_bits,
                    &mut last_bits,
                    &mut had_any,
                ) {
                    return MoltObject::none().bits();
                }
            } else {
                profile_hit_unchecked(&SPLIT_WS_UNICODE_PATH_COUNT);
                let Ok(line_str) = std::str::from_utf8(line_bytes) else {
                    return MoltObject::none().bits();
                };
                for part in line_str.split_whitespace() {
                    if !dict_inc_with_string_token(
                        _py,
                        dict_ptr,
                        part.as_bytes(),
                        delta_bits,
                        &mut last_bits,
                        &mut had_any,
                    ) {
                        return MoltObject::none().bits();
                    }
                }
            }
            split_dict_inc_result_tuple(_py, last_bits, had_any)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_split_sep_dict_inc(
    line_bits: u64,
    sep_bits: u64,
    dict_bits: u64,
    delta_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let line_obj = obj_from_bits(line_bits);
        let sep_obj = obj_from_bits(sep_bits);
        let dict_obj = obj_from_bits(dict_bits);
        let Some(line_ptr) = line_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "split expects str");
        };
        let Some(sep_ptr) = sep_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "must be str or None");
        };
        let Some(dict_ptr_raw) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
        };
        unsafe {
            if object_type_id(line_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "split expects str");
            }
            if object_type_id(sep_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "must be str or None");
            }
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, dict_ptr_raw) else {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "dict increment expects dict");
            }

            let line_bytes =
                std::slice::from_raw_parts(string_bytes(line_ptr), string_len(line_ptr));
            let sep_bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
            if sep_bytes.is_empty() {
                return raise_exception::<_>(_py, "ValueError", "empty separator");
            }
            let mut last_bits = MoltObject::none().bits();
            let mut had_any = false;
            let mut start = 0usize;
            if sep_bytes.len() == 1 {
                for idx in memchr::memchr_iter(sep_bytes[0], line_bytes) {
                    if !dict_inc_with_string_token(
                        _py,
                        dict_ptr,
                        &line_bytes[start..idx],
                        delta_bits,
                        &mut last_bits,
                        &mut had_any,
                    ) {
                        return MoltObject::none().bits();
                    }
                    start = idx + 1;
                }
            } else {
                let finder = memmem::Finder::new(sep_bytes);
                for idx in finder.find_iter(line_bytes) {
                    if !dict_inc_with_string_token(
                        _py,
                        dict_ptr,
                        &line_bytes[start..idx],
                        delta_bits,
                        &mut last_bits,
                        &mut had_any,
                    ) {
                        return MoltObject::none().bits();
                    }
                    start = idx + sep_bytes.len();
                }
            }
            if !dict_inc_with_string_token(
                _py,
                dict_ptr,
                &line_bytes[start..],
                delta_bits,
                &mut last_bits,
                &mut had_any,
            ) {
                return MoltObject::none().bits();
            }
            split_dict_inc_result_tuple(_py, last_bits, had_any)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_taq_ingest_line(
    dict_bits: u64,
    line_bits: u64,
    bucket_size_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        profile_hit_unchecked(&TAQ_INGEST_CALL_COUNT);
        let dict_obj = obj_from_bits(dict_bits);
        let line_obj = obj_from_bits(line_bits);
        let Some(dict_ptr_raw) = dict_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "TAQ ingest expects dict");
        };
        let Some(line_ptr) = line_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "TAQ ingest expects str");
        };
        let Some(bucket_size) = obj_from_bits(bucket_size_bits).as_int() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "TAQ ingest expects integer bucket size",
            );
        };
        if bucket_size == 0 {
            return raise_exception::<_>(
                _py,
                "ZeroDivisionError",
                "integer division or modulo by zero",
            );
        }
        unsafe {
            if object_type_id(line_ptr) != TYPE_ID_STRING {
                return raise_exception::<_>(_py, "TypeError", "TAQ ingest expects str");
            }
            let Some(dict_bits) = dict_like_bits_from_ptr(_py, dict_ptr_raw) else {
                return raise_exception::<_>(_py, "TypeError", "TAQ ingest expects dict");
            };
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                return MoltObject::none().bits();
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return raise_exception::<_>(_py, "TypeError", "TAQ ingest expects dict");
            }

            let line_bytes =
                std::slice::from_raw_parts(string_bytes(line_ptr), string_len(line_ptr));
            let mut field_idx = 0usize;
            let mut field_start = 0usize;
            let mut ts_field: Option<&[u8]> = None;
            let mut sym_field: Option<&[u8]> = None;
            let mut vol_field: Option<&[u8]> = None;
            for idx in 0..=line_bytes.len() {
                if idx == line_bytes.len() || line_bytes[idx] == b'|' {
                    let field = &line_bytes[field_start..idx];
                    match field_idx {
                        0 => ts_field = Some(field),
                        2 => sym_field = Some(field),
                        4 => {
                            vol_field = Some(field);
                            break;
                        }
                        _ => {}
                    }
                    field_idx += 1;
                    field_start = idx + 1;
                }
            }
            let Some(ts_field) = ts_field else {
                return raise_exception::<_>(_py, "IndexError", "list index out of range");
            };
            let Some(sym_field) = sym_field else {
                return raise_exception::<_>(_py, "IndexError", "list index out of range");
            };
            let Some(vol_field) = vol_field else {
                return raise_exception::<_>(_py, "IndexError", "list index out of range");
            };
            if ts_field == b"END" || vol_field == b"ENDP" {
                profile_hit_unchecked(&TAQ_INGEST_SKIP_MARKER_COUNT);
                return MoltObject::from_bool(false).bits();
            }
            let Some(timestamp) = parse_ascii_i64_field(_py, ts_field) else {
                return MoltObject::none().bits();
            };
            let Some(volume) = parse_ascii_i64_field(_py, vol_field) else {
                return MoltObject::none().bits();
            };
            let Some(series_bits) =
                dict_setdefault_empty_list_with_string_token(_py, dict_ptr, sym_field)
            else {
                return MoltObject::none().bits();
            };
            let bucket_bits = MoltObject::from_int(timestamp.div_euclid(bucket_size)).bits();
            let volume_bits = MoltObject::from_int(volume).bits();
            let pair_ptr = alloc_tuple(_py, &[bucket_bits, volume_bits]);
            if pair_ptr.is_null() {
                dec_ref_bits(_py, series_bits);
                return MoltObject::none().bits();
            }
            let pair_bits = MoltObject::from_ptr(pair_ptr).bits();
            let appended = if let Some(series_ptr) = obj_from_bits(series_bits).as_ptr() {
                if object_type_id(series_ptr) == TYPE_ID_LIST {
                    let _ = molt_list_append(series_bits, pair_bits);
                    !exception_pending(_py)
                } else {
                    let Some(append_name_bits) = attr_name_bits_from_bytes(_py, b"append") else {
                        dec_ref_bits(_py, pair_bits);
                        dec_ref_bits(_py, series_bits);
                        return MoltObject::none().bits();
                    };
                    let method_bits = attr_lookup_ptr(_py, series_ptr, append_name_bits);
                    dec_ref_bits(_py, append_name_bits);
                    let Some(method_bits) = method_bits else {
                        dec_ref_bits(_py, pair_bits);
                        dec_ref_bits(_py, series_bits);
                        return MoltObject::none().bits();
                    };
                    let out_bits = call_callable1(_py, method_bits, pair_bits);
                    if maybe_ptr_from_bits(out_bits).is_some() {
                        dec_ref_bits(_py, out_bits);
                    }
                    !exception_pending(_py)
                }
            } else {
                false
            };
            dec_ref_bits(_py, pair_bits);
            dec_ref_bits(_py, series_bits);
            if !appended {
                return MoltObject::none().bits();
            }
            MoltObject::from_bool(true).bits()
        }
    })
}

pub(crate) fn dict_table_capacity(entries: usize) -> usize {
    let mut cap = entries.saturating_mul(2).next_power_of_two();
    if cap < 8 {
        cap = 8;
    }
    cap
}

const TABLE_TOMBSTONE: usize = usize::MAX;

#[inline]
unsafe fn reserve_dict_order(_py: &PyToken<'_>, order: &mut Vec<u64>, additional: usize) -> bool {
    let Some(required_len) = order.len().checked_add(additional) else {
        let _ = raise_exception::<u64>(_py, "MemoryError", "dict allocation failed");
        return false;
    };
    unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            order as *mut Vec<u64>,
            required_len,
            "dict allocation failed",
        )
    }
}

#[inline]
unsafe fn reserve_set_order(_py: &PyToken<'_>, order: &mut Vec<u64>, additional: usize) -> bool {
    let Some(required_len) = order.len().checked_add(additional) else {
        let _ = raise_exception::<u64>(_py, "MemoryError", "set allocation failed");
        return false;
    };
    unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            order as *mut Vec<u64>,
            required_len,
            "set allocation failed",
        )
    }
}

#[inline]
unsafe fn reserve_hashes(
    _py: &PyToken<'_>,
    hashes: &mut Vec<u64>,
    additional: usize,
    message: &'static str,
) -> bool {
    let Some(required_len) = hashes.len().checked_add(additional) else {
        let _ = raise_exception::<u64>(_py, "MemoryError", message);
        return false;
    };
    unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            hashes as *mut Vec<u64>,
            required_len,
            message,
        )
    }
}

fn dict_insert_entry(_py: &PyToken<'_>, hashes: &[u64], table: &mut [usize], entry_idx: usize) {
    let mask = table.len() - 1;
    let hash = hashes[entry_idx];
    let mut slot = (hash as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}

pub(crate) fn dict_insert_entry_with_hash(
    _py: &PyToken<'_>,
    _order: &[u64],
    table: &mut [usize],
    entry_idx: usize,
    hash: u64,
) {
    let mask = table.len() - 1;
    let mut slot = (hash as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}
pub(crate) fn dict_rebuild(
    _py: &PyToken<'_>,
    order: &[u64],
    hashes: &[u64],
    table: &mut Vec<usize>,
    capacity: usize,
) {
    if !unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            table as *mut Vec<usize>,
            capacity,
            "dict allocation failed",
        )
    } {
        return;
    }
    table.clear();
    table.resize(capacity, 0);
    let entry_count = order.len() / 2;
    for entry_idx in 0..entry_count {
        dict_insert_entry(_py, hashes, table, entry_idx);
    }
}

pub(crate) fn dict_find_entry_fast(
    _py: &PyToken<'_>,
    order: &[u64],
    hashes: &[u64],
    table: &[usize],
    key_bits: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let hash = hash_bits(_py, key_bits);
    let mut slot = (hash as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        if entry_idx * 2 >= order.len() {
            slot = (slot + 1) & mask;
            continue;
        }
        if hashes.get(entry_idx).copied() != Some(hash) {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_key = order[entry_idx * 2];
        // Fast path: identical bit patterns are always equal.
        if entry_key == key_bits || obj_eq(_py, obj_from_bits(entry_key), obj_from_bits(key_bits)) {
            return Some(entry_idx);
        }
        slot = (slot + 1) & mask;
    }
}

pub(crate) fn dict_find_entry(
    _py: &PyToken<'_>,
    order: &[u64],
    hashes: &[u64],
    table: &[usize],
    key_bits: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let pending_before = exception_pending(_py);
    let mask = table.len() - 1;
    let hash = hash_bits(_py, key_bits);
    let mut slot = (hash as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        // Safety: corrupted hash tables can have huge entry values from
        // use-after-free. Bounds-check before indexing to turn a crash
        // into a graceful "not found".
        if entry_idx * 2 >= order.len() {
            // Corrupted entry — skip it like a tombstone.
            slot = (slot + 1) & mask;
            continue;
        }
        if hashes.get(entry_idx).copied() != Some(hash) {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_key = order[entry_idx * 2];
        // Fast path: identical bit patterns are always equal.
        if entry_key == key_bits {
            return Some(entry_idx);
        }
        if let Some(eq) = unsafe { string_bits_eq(entry_key, key_bits) } {
            if eq {
                return Some(entry_idx);
            }
            slot = (slot + 1) & mask;
            continue;
        }
        let eq = unsafe { eq_bool_from_bits(_py, entry_key, key_bits) };
        match eq {
            Some(true) => return Some(entry_idx),
            Some(false) => {
                if pending_before && unsafe { string_bits_eq(entry_key, key_bits) } == Some(true) {
                    return Some(entry_idx);
                }
            }
            None => {
                if pending_before && unsafe { string_bits_eq(entry_key, key_bits) } == Some(true) {
                    return Some(entry_idx);
                }
                return None;
            }
        }
        slot = (slot + 1) & mask;
    }
}

// ---------------------------------------------------------------------------
// SIMD-accelerated byte-level equality for string/bytes comparisons.
// For short strings (< 32 bytes), the compiler-generated memcmp is fast enough.
// For longer strings, explicit SIMD provides measurable wins especially on
// Apple Silicon where NEON is always available with no runtime detection cost.
// ---------------------------------------------------------------------------

/// SIMD byte equality: returns true if `a[..len] == b[..len]`.
/// Precondition: both pointers are valid for `len` bytes.
#[inline(always)]
pub(in crate::object) unsafe fn simd_bytes_eq(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        // Tiny strings (<=8 bytes): direct comparison, no SIMD overhead.
        if len <= 8 {
            if len == 0 {
                return true;
            }
            return std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len);
        }

        // Short strings (9-15 bytes): compare overlapping 8-byte windows.
        // This covers the full range without underflowing the tail pointer.
        if len < 16 {
            return simd_bytes_eq_short_u64(a, b, len);
        }

        // Short strings (16-31 bytes): use NEON/SSE2 16-byte loads instead of
        // scalar memcmp. Two overlapping 16-byte loads cover any length in
        // 16..31 without a loop, which is measurably faster for dict-key
        // comparisons where keys are typically short identifiers (< 32 bytes).
        #[cfg(target_arch = "aarch64")]
        if len < 32 {
            return simd_bytes_eq_short_neon(a, b, len);
        }
        #[cfg(target_arch = "x86_64")]
        if len < 32 {
            if std::arch::is_x86_feature_detected!("sse2") {
                return simd_bytes_eq_short_sse2(a, b, len);
            }
            return std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len);
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
        if len < 32 {
            return std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len);
        }

        // Long strings (>= 32 bytes): full SIMD loops.
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx2") {
                return simd_bytes_eq_avx2(a, b, len);
            }
            return simd_bytes_eq_sse2(a, b, len);
        }
        #[cfg(target_arch = "aarch64")]
        {
            return simd_bytes_eq_neon(a, b, len);
        }
        #[cfg(target_arch = "wasm32")]
        {
            return simd_bytes_eq_wasm32(a, b, len);
        }
        #[allow(unreachable_code)]
        {
            std::slice::from_raw_parts(a, len) == std::slice::from_raw_parts(b, len)
        }
    }
}

/// Short-string equality for 9-15 bytes: overlapping unaligned 8-byte loads.
#[inline(always)]
unsafe fn simd_bytes_eq_short_u64(a: *const u8, b: *const u8, len: usize) -> bool {
    debug_assert!((9..16).contains(&len));
    unsafe {
        let head_a = std::ptr::read_unaligned(a as *const u64);
        let head_b = std::ptr::read_unaligned(b as *const u64);
        if head_a != head_b {
            return false;
        }
        let tail_a = std::ptr::read_unaligned(a.add(len - 8) as *const u64);
        let tail_b = std::ptr::read_unaligned(b.add(len - 8) as *const u64);
        tail_a == tail_b
    }
}

/// NEON short-string equality for 16-31 bytes: two overlapping 16-byte loads.
#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn simd_bytes_eq_short_neon(a: *const u8, b: *const u8, len: usize) -> bool {
    use std::arch::aarch64::*;
    debug_assert!((16..32).contains(&len));
    unsafe {
        // Load from the start
        let va0 = vld1q_u8(a);
        let vb0 = vld1q_u8(b);
        let cmp0 = vceqq_u8(va0, vb0);
        // Load from (end - 16), overlapping with the first load for short strings
        let va1 = vld1q_u8(a.add(len - 16));
        let vb1 = vld1q_u8(b.add(len - 16));
        let cmp1 = vceqq_u8(va1, vb1);
        // Both loads must match: AND the comparison results and check all-0xFF
        let combined = vandq_u8(cmp0, cmp1);
        vminvq_u8(combined) == 0xFF
    }
}

/// SSE2 short-string equality for 16-31 bytes: two overlapping 16-byte loads.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn simd_bytes_eq_short_sse2(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::x86_64::*;
        debug_assert!((16..32).contains(&len));
        // Load from the start
        let va0 = _mm_loadu_si128(a as *const __m128i);
        let vb0 = _mm_loadu_si128(b as *const __m128i);
        let cmp0 = _mm_cmpeq_epi8(va0, vb0);
        // Load from (end - 16), overlapping with the first load
        let va1 = _mm_loadu_si128(a.add(len - 16) as *const __m128i);
        let vb1 = _mm_loadu_si128(b.add(len - 16) as *const __m128i);
        let cmp1 = _mm_cmpeq_epi8(va1, vb1);
        // Both must be all-equal: AND the masks
        let mask0 = _mm_movemask_epi8(cmp0);
        let mask1 = _mm_movemask_epi8(cmp1);
        (mask0 & mask1) == 0xFFFF
    }
}

// ---------------------------------------------------------------------------
// SIMD-accelerated u64 linear scan for list/tuple `in` operator.
// For NaN-boxed integers, bools, and None, bit-equality implies value equality,
// so we can scan the raw u64 element slice without calling eq_bool_from_bits.
// On aarch64 (NEON) and x86_64 (SSE2/AVX2), we broadcast the needle into a
// SIMD register and compare 2-4 elements per cycle.
// ---------------------------------------------------------------------------

/// Returns true if `needle` appears in `haystack` by raw u64 identity.
/// Only valid when the needle is a NaN-boxed int, bool, or None (where
/// bit-equality implies Python value equality).
#[inline(always)]
pub(in crate::object) fn simd_contains_u64(haystack: &[u64], needle: u64) -> bool {
    let len = haystack.len();
    if len == 0 {
        return false;
    }

    #[cfg(target_arch = "aarch64")]
    {
        return unsafe { simd_contains_u64_neon(haystack, needle) };
    }

    #[cfg(target_arch = "x86_64")]
    {
        if len >= 4 && std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { simd_contains_u64_avx2(haystack, needle) };
        }
        if len >= 2 && std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { simd_contains_u64_sse2(haystack, needle) };
        }
    }

    // Scalar fallback (also covers wasm32 and other targets).
    #[allow(unreachable_code)]
    {
        for &elem in haystack {
            if elem == needle {
                return true;
            }
        }
        false
    }
}

#[cfg(target_arch = "aarch64")]
#[inline(always)]
unsafe fn simd_contains_u64_neon(haystack: &[u64], needle: u64) -> bool {
    unsafe {
        use std::arch::aarch64::*;
        let ptr = haystack.as_ptr();
        let len = haystack.len();
        let needle_vec = vdupq_n_u64(needle);
        let mut i = 0usize;
        // Process 2 u64s at a time (128-bit NEON register = 2 x u64).
        while i + 2 <= len {
            let chunk = vld1q_u64(ptr.add(i));
            let cmp = vceqq_u64(chunk, needle_vec);
            // vmaxvq_u64 is not available; use vgetq_lane to check both lanes.
            if vgetq_lane_u64(cmp, 0) != 0 || vgetq_lane_u64(cmp, 1) != 0 {
                return true;
            }
            i += 2;
        }
        // Tail element
        if i < len && *ptr.add(i) == needle {
            return true;
        }
        false
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn simd_contains_u64_sse2(haystack: &[u64], needle: u64) -> bool {
    unsafe {
        use std::arch::x86_64::*;
        let ptr = haystack.as_ptr() as *const __m128i;
        let len = haystack.len();
        let needle_vec = _mm_set1_epi64x(needle as i64);
        let mut i = 0usize;
        // Process 2 u64s at a time (128-bit register = 2 x u64).
        while i + 2 <= len {
            let chunk = _mm_loadu_si128(ptr.add(i / 2));
            let cmp = _mm_cmpeq_epi32(chunk, needle_vec);
            // For 64-bit equality, both 32-bit halves must match.
            // Shuffle to align adjacent 32-bit results and AND them.
            let shuffled = _mm_shuffle_epi32(cmp, 0b10_11_00_01);
            let both = _mm_and_si128(cmp, shuffled);
            if _mm_movemask_epi8(both) != 0 {
                return true;
            }
            i += 2;
        }
        if i < len && haystack[i] == needle {
            return true;
        }
        false
    }
}

#[cfg(target_arch = "x86_64")]
#[inline(always)]
unsafe fn simd_contains_u64_avx2(haystack: &[u64], needle: u64) -> bool {
    unsafe {
        use std::arch::x86_64::*;
        let ptr = haystack.as_ptr() as *const __m256i;
        let len = haystack.len();
        let needle_vec = _mm256_set1_epi64x(needle as i64);
        let mut i = 0usize;
        // Process 4 u64s at a time (256-bit register = 4 x u64).
        while i + 4 <= len {
            let chunk = _mm256_loadu_si256(ptr.add(i / 4));
            let cmp = _mm256_cmpeq_epi64(chunk, needle_vec);
            if _mm256_movemask_epi8(cmp) != 0 {
                return true;
            }
            i += 4;
        }
        // Tail: scalar check for remaining 0-3 elements.
        while i < len {
            if haystack[i] == needle {
                return true;
            }
            i += 1;
        }
        false
    }
}

#[cfg(target_arch = "wasm32")]
#[inline]
unsafe fn simd_bytes_eq_wasm32(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = 0usize;
        while i + 16 <= len {
            let va = v128_load(a.add(i) as *const v128);
            let vb = v128_load(b.add(i) as *const v128);
            let cmp = u8x16_eq(va, vb);
            if u8x16_bitmask(cmp) != 0xFFFF {
                return false;
            }
            i += 16;
        }
        std::slice::from_raw_parts(a.add(i), len - i)
            == std::slice::from_raw_parts(b.add(i), len - i)
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_bytes_eq_sse2(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::x86_64::*;
        let mut i = 0usize;
        while i + 16 <= len {
            let va = _mm_loadu_si128(a.add(i) as *const __m128i);
            let vb = _mm_loadu_si128(b.add(i) as *const __m128i);
            let cmp = _mm_cmpeq_epi8(va, vb);
            if _mm_movemask_epi8(cmp) != 0xFFFF {
                return false;
            }
            i += 16;
        }
        // Tail: compare remaining bytes
        std::slice::from_raw_parts(a.add(i), len - i)
            == std::slice::from_raw_parts(b.add(i), len - i)
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_bytes_eq_avx2(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::x86_64::*;
        let mut i = 0usize;
        while i + 32 <= len {
            let va = _mm256_loadu_si256(a.add(i) as *const __m256i);
            let vb = _mm256_loadu_si256(b.add(i) as *const __m256i);
            let cmp = _mm256_cmpeq_epi8(va, vb);
            if _mm256_movemask_epi8(cmp) != -1i32 {
                return false;
            }
            i += 32;
        }
        // SSE2 tail for 16-byte remainder
        if i + 16 <= len {
            let va = _mm_loadu_si128(a.add(i) as *const __m128i);
            let vb = _mm_loadu_si128(b.add(i) as *const __m128i);
            let cmp = _mm_cmpeq_epi8(va, vb);
            if _mm_movemask_epi8(cmp) != 0xFFFF {
                return false;
            }
            i += 16;
        }
        std::slice::from_raw_parts(a.add(i), len - i)
            == std::slice::from_raw_parts(b.add(i), len - i)
    }
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn simd_bytes_eq_neon(a: *const u8, b: *const u8, len: usize) -> bool {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        while i + 16 <= len {
            let va = vld1q_u8(a.add(i));
            let vb = vld1q_u8(b.add(i));
            let cmp = vceqq_u8(va, vb);
            // vminvq_u8 returns 0xFF if all lanes equal, < 0xFF if any differ
            if vminvq_u8(cmp) != 0xFF {
                return false;
            }
            i += 16;
        }
        std::slice::from_raw_parts(a.add(i), len - i)
            == std::slice::from_raw_parts(b.add(i), len - i)
    }
}

unsafe fn string_bits_eq(a_bits: u64, b_bits: u64) -> Option<bool> {
    unsafe {
        let a_obj = obj_from_bits(a_bits);
        let b_obj = obj_from_bits(b_bits);
        let a_ptr = a_obj.as_ptr()?;
        let b_ptr = b_obj.as_ptr()?;
        if object_type_id(a_ptr) != TYPE_ID_STRING || object_type_id(b_ptr) != TYPE_ID_STRING {
            return None;
        }
        if a_ptr == b_ptr {
            return Some(true);
        }
        let a_len = string_len(a_ptr);
        let b_len = string_len(b_ptr);
        if a_len != b_len {
            return Some(false);
        }
        Some(simd_bytes_eq(
            string_bytes(a_ptr),
            string_bytes(b_ptr),
            a_len,
        ))
    }
}

pub(crate) fn dict_find_entry_with_hash(
    _py: &PyToken<'_>,
    order: &[u64],
    hashes: &[u64],
    table: &[usize],
    key_bits: u64,
    hash: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let mut slot = (hash as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        if entry_idx * 2 >= order.len() {
            slot = (slot + 1) & mask;
            continue;
        }
        if hashes.get(entry_idx).copied() != Some(hash) {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_key = order[entry_idx * 2];
        // Fast path: identical bit patterns are always equal.
        if entry_key == key_bits {
            return Some(entry_idx);
        }
        if let Some(eq) = unsafe { string_bits_eq(entry_key, key_bits) } {
            if eq {
                return Some(entry_idx);
            }
            slot = (slot + 1) & mask;
            continue;
        }
        let eq = unsafe { eq_bool_from_bits(_py, entry_key, key_bits) };
        match eq {
            Some(true) => return Some(entry_idx),
            Some(false) => {}
            None => return None,
        }
        slot = (slot + 1) & mask;
    }
}

pub(crate) fn set_table_capacity(entries: usize) -> usize {
    dict_table_capacity(entries)
}

fn set_insert_entry(_py: &PyToken<'_>, hashes: &[u64], table: &mut [usize], entry_idx: usize) {
    let mask = table.len() - 1;
    let mut slot = (hashes[entry_idx] as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}

fn set_insert_entry_with_hash(
    _py: &PyToken<'_>,
    _order: &[u64],
    table: &mut [usize],
    entry_idx: usize,
    hash: u64,
) {
    let mask = table.len() - 1;
    let mut slot = (hash as usize) & mask;
    let mut first_tombstone = None;
    loop {
        let entry = table[slot];
        if entry == 0 {
            let target = first_tombstone.unwrap_or(slot);
            table[target] = entry_idx + 1;
            return;
        }
        if entry == TABLE_TOMBSTONE && first_tombstone.is_none() {
            first_tombstone = Some(slot);
        }
        slot = (slot + 1) & mask;
    }
}
pub(in crate::object) fn set_rebuild(
    _py: &PyToken<'_>,
    order: &[u64],
    hashes: &[u64],
    table: &mut Vec<usize>,
    capacity: usize,
) {
    crate::gil_assert();
    if !unsafe {
        crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            table as *mut Vec<usize>,
            capacity,
            "set allocation failed",
        )
    } {
        return;
    }
    table.clear();
    table.resize(capacity, 0);
    for entry_idx in 0..order.len() {
        set_insert_entry(_py, hashes, table, entry_idx);
    }
}

pub(crate) fn set_find_entry_fast(
    _py: &PyToken<'_>,
    order: &[u64],
    hashes: &[u64],
    table: &[usize],
    key_bits: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let hash = hash_bits(_py, key_bits);
    let mut slot = (hash as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        if hashes.get(entry_idx).copied() != Some(hash) {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_key = order[entry_idx];
        // Identity check first (CPython semantics: `x is y or x == y`).
        if entry_key == key_bits {
            return Some(entry_idx);
        }
        if obj_eq(_py, obj_from_bits(entry_key), obj_from_bits(key_bits)) {
            return Some(entry_idx);
        }
        slot = (slot + 1) & mask;
    }
}

pub(crate) fn set_find_entry(
    _py: &PyToken<'_>,
    order: &[u64],
    hashes: &[u64],
    table: &[usize],
    key_bits: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let hash = hash_bits(_py, key_bits);
    let mut slot = (hash as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        if hashes.get(entry_idx).copied() != Some(hash) {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_key = order[entry_idx];
        // Identity check first (CPython semantics: `x is y or x == y`).
        if entry_key == key_bits {
            return Some(entry_idx);
        }
        let eq = unsafe { eq_bool_from_bits(_py, entry_key, key_bits) };
        match eq {
            Some(true) => return Some(entry_idx),
            Some(false) => {}
            None => return None,
        }
        slot = (slot + 1) & mask;
    }
}

pub(crate) fn set_find_entry_with_hash(
    _py: &PyToken<'_>,
    order: &[u64],
    hashes: &[u64],
    table: &[usize],
    key_bits: u64,
    hash: u64,
) -> Option<usize> {
    if table.is_empty() {
        return None;
    }
    let mask = table.len() - 1;
    let mut slot = (hash as usize) & mask;
    loop {
        let entry = table[slot];
        if entry == 0 {
            return None;
        }
        if entry == TABLE_TOMBSTONE {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_idx = entry - 1;
        if hashes.get(entry_idx).copied() != Some(hash) {
            slot = (slot + 1) & mask;
            continue;
        }
        let entry_key = order[entry_idx];
        // Identity check first (CPython semantics: `x is y or x == y`).
        if entry_key == key_bits {
            return Some(entry_idx);
        }
        let eq = unsafe { eq_bool_from_bits(_py, entry_key, key_bits) };
        match eq {
            Some(true) => return Some(entry_idx),
            Some(false) => {}
            None => return None,
        }
        slot = (slot + 1) & mask;
    }
}

pub(in crate::object) fn concat_bytes_like(
    _py: &PyToken<'_>,
    left: &[u8],
    right: &[u8],
    type_id: u32,
) -> Option<u64> {
    let total = left.len().checked_add(right.len())?;
    if type_id == TYPE_ID_BYTEARRAY {
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(left);
        out.extend_from_slice(right);
        let ptr = alloc_bytearray(_py, &out);
        if ptr.is_null() {
            return None;
        }
        return Some(MoltObject::from_ptr(ptr).bits());
    }
    let ptr = alloc_bytes_like_with_len(_py, total, type_id);
    if ptr.is_null() {
        return None;
    }
    unsafe {
        let data_ptr = ptr.add(std::mem::size_of::<usize>());
        std::ptr::copy_nonoverlapping(left.as_ptr(), data_ptr, left.len());
        std::ptr::copy_nonoverlapping(right.as_ptr(), data_ptr.add(left.len()), right.len());
    }
    Some(MoltObject::from_ptr(ptr).bits())
}

pub(in crate::object) fn fill_repeated_bytes(dst: &mut [u8], pattern: &[u8]) {
    if pattern.is_empty() {
        return;
    }
    if pattern.len() == 1 {
        dst.fill(pattern[0]);
        return;
    }
    let mut filled = pattern.len().min(dst.len());
    dst[..filled].copy_from_slice(&pattern[..filled]);
    while filled < dst.len() {
        let copy_len = std::cmp::min(filled, dst.len() - filled);
        let (head, tail) = dst.split_at_mut(filled);
        tail[..copy_len].copy_from_slice(&head[..copy_len]);
        filled += copy_len;
    }
}

pub(crate) unsafe fn dict_set_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) {
    unsafe {
        crate::gil_assert();
        // Fast path: inline NaN-boxed ints bypass all exception checks,
        // hashability validation, and refcounting overhead.
        let key_obj = obj_from_bits(key_bits);
        if let Some(i) = key_obj.as_int() {
            return dict_set_inline_int_in_place(_py, ptr, key_bits, i, val_bits);
        }
        let hash = if key_obj.as_ptr().is_none() {
            // Bool, None, or other inline -- still always hashable, use
            // the normal hash path but skip ensure_hashable.
            hash_bits(_py, key_bits)
        } else {
            // Heap-allocated key: need full hashability check.
            if !ensure_hashable(_py, key_bits, HashContext::DictKey) {
                return;
            }
            hash_bits(_py, key_bits)
        };
        if exception_pending(_py) {
            return;
        }
        let order = dict_order(ptr);
        let hashes = dict_hashes(ptr);
        let table = dict_table(ptr);
        let found = dict_find_entry_with_hash(_py, order, hashes, table, key_bits, hash);
        if exception_pending(_py) {
            return;
        }
        if let Some(entry_idx) = found {
            let val_idx = entry_idx * 2 + 1;
            let old_bits = order[val_idx];
            if old_bits != val_bits {
                if crate::object::refcount_opt::is_heap_ref(val_bits) {
                    inc_ref_bits(_py, val_bits);
                    (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
                }
                order[val_idx] = val_bits;
                if crate::object::refcount_opt::is_heap_ref(old_bits) {
                    dec_ref_bits(_py, old_bits);
                }
            }
            return;
        }

        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, hashes, table, capacity);
            if exception_pending(_py) {
                return;
            }
        }

        if !reserve_dict_order(_py, order, 2)
            || !reserve_hashes(_py, hashes, 1, "dict allocation failed")
        {
            return;
        }
        order.push(key_bits);
        order.push(val_bits);
        hashes.push(hash);
        if crate::object::refcount_opt::is_heap_ref(key_bits) {
            inc_ref_bits(_py, key_bits);
        }
        if crate::object::refcount_opt::is_heap_ref(val_bits) {
            inc_ref_bits(_py, val_bits);
        }
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        if crate::object::refcount_opt::is_heap_ref(key_bits)
            || crate::object::refcount_opt::is_heap_ref(val_bits)
        {
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
    }
}

/// Ultra-fast dict set for inline NaN-boxed integer keys AND values.
/// Skips: ensure_hashable (ints always hashable), exception_pending checks
/// (hash_int + bit-equality cannot raise), and inc_ref/dec_ref (inline
/// values have no heap allocation).
#[inline]
pub(crate) unsafe fn dict_set_inline_int_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    key_int: i64,
    val_bits: u64,
) {
    unsafe {
        let hash = hash_int(key_int) as u64;
        let order = dict_order(ptr);
        let hashes = dict_hashes(ptr);
        let table = dict_table(ptr);

        // Inline find: for integer keys, bit-equality is sufficient.
        if !table.is_empty() {
            let mask = table.len() - 1;
            let mut slot = (hash as usize) & mask;
            loop {
                let entry = table[slot];
                if entry == 0 {
                    break;
                }
                if entry != TABLE_TOMBSTONE {
                    let entry_idx = entry - 1;
                    if hashes.get(entry_idx).copied() == Some(hash)
                        && order[entry_idx * 2] == key_bits
                    {
                        // Key exists -- update value in place.
                        let val_idx = entry_idx * 2 + 1;
                        let old_bits = order[val_idx];
                        if old_bits != val_bits {
                            let old_obj = obj_from_bits(old_bits);
                            let new_obj = obj_from_bits(val_bits);
                            if new_obj.as_ptr().is_some() {
                                inc_ref_bits(_py, val_bits);
                                (*header_from_obj_ptr(ptr)).flags |=
                                    crate::object::HEADER_FLAG_CONTAINS_REFS;
                            }
                            order[val_idx] = val_bits;
                            if old_obj.as_ptr().is_some() {
                                dec_ref_bits(_py, old_bits);
                            }
                        }
                        return;
                    }
                }
                slot = (slot + 1) & mask;
            }
        }

        // Key not found: insert.
        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, hashes, table, capacity);
        }

        if !reserve_dict_order(_py, order, 2)
            || !reserve_hashes(_py, hashes, 1, "dict allocation failed")
        {
            return;
        }
        order.push(key_bits);
        order.push(val_bits);
        hashes.push(hash);
        // key is inline int: no refcount needed.
        // value: only inc_ref if heap-allocated.
        let val_obj = obj_from_bits(val_bits);
        if val_obj.as_ptr().is_some() {
            inc_ref_bits(_py, val_bits);
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
    }
}

/// Ultra-fast dict get for inline NaN-boxed integer keys.
/// Skips: ensure_hashable, exception state save/restore, and the
/// string_bits_eq / eq_bool_from_bits fallback paths.
#[inline]
pub(crate) unsafe fn dict_get_inline_int_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    key_int: i64,
) -> Option<u64> {
    unsafe {
        let hash = hash_int(key_int) as u64;
        let order = dict_order(ptr);
        let hashes = dict_hashes(ptr);
        let table = dict_table(ptr);
        if table.is_empty() {
            return None;
        }
        let mask = table.len() - 1;
        let mut slot = (hash as usize) & mask;
        loop {
            let entry = table[slot];
            if entry == 0 {
                return None;
            }
            if entry != TABLE_TOMBSTONE {
                let entry_idx = entry - 1;
                if hashes.get(entry_idx).copied() == Some(hash) && order[entry_idx * 2] == key_bits
                {
                    return Some(order[entry_idx * 2 + 1]);
                }
            }
            slot = (slot + 1) & mask;
        }
    }
}

#[allow(dead_code)]
pub(crate) unsafe fn dict_set_in_place_preserving_pending(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    val_bits: u64,
) {
    unsafe {
        crate::gil_assert();
        if !ensure_hashable(_py, key_bits, HashContext::DictKey) {
            return;
        }
        let pending_before = exception_pending(_py);
        let prev_exc_bits = if pending_before {
            exception_last_bits_noinc(_py).unwrap_or(0)
        } else {
            0
        };
        let hash = hash_bits(_py, key_bits);
        if exception_pending(_py) {
            if !pending_before {
                return;
            }
            let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
            if after_exc_bits != prev_exc_bits {
                return;
            }
        }
        let order = dict_order(ptr);
        let hashes = dict_hashes(ptr);
        let table = dict_table(ptr);
        let found = dict_find_entry_with_hash(_py, order, hashes, table, key_bits, hash);
        if exception_pending(_py) {
            if !pending_before {
                return;
            }
            let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
            if after_exc_bits != prev_exc_bits {
                return;
            }
        }
        if let Some(entry_idx) = found {
            let val_idx = entry_idx * 2 + 1;
            let old_bits = order[val_idx];
            if old_bits != val_bits {
                inc_ref_bits(_py, val_bits);
                order[val_idx] = val_bits;
                if crate::object::refcount_opt::is_heap_ref(val_bits) {
                    (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
                }
                dec_ref_bits(_py, old_bits);
            }
            return;
        }

        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, hashes, table, capacity);
            if exception_pending(_py) {
                if !pending_before {
                    return;
                }
                let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
                if after_exc_bits != prev_exc_bits {
                    return;
                }
            }
        }

        let Some(required_len) = order.len().checked_add(2) else {
            if !pending_before {
                let _ = raise_exception::<u64>(_py, "MemoryError", "dict allocation failed");
            }
            return;
        };
        if !crate::object::backing::tracked_vec_reserve_for_len(
            order as *mut Vec<u64>,
            required_len,
        ) {
            if !pending_before {
                let _ = raise_exception::<u64>(_py, "MemoryError", "dict allocation failed");
            }
            return;
        }
        if !reserve_hashes(_py, hashes, 1, "dict allocation failed") {
            return;
        }
        order.push(key_bits);
        order.push(val_bits);
        hashes.push(hash);
        inc_ref_bits(_py, key_bits);
        inc_ref_bits(_py, val_bits);
        let entry_idx = order.len() / 2 - 1;
        dict_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        if crate::object::refcount_opt::is_heap_ref(key_bits)
            || crate::object::refcount_opt::is_heap_ref(val_bits)
        {
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
    }
}

pub(crate) unsafe fn set_add_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
    ctx: HashContext,
) {
    unsafe {
        crate::gil_assert();
        if !ensure_hashable(_py, key_bits, ctx) {
            return;
        }
        let hash = hash_bits(_py, key_bits);
        if exception_pending(_py) {
            return;
        }
        let order = set_order(ptr);
        let hashes = set_hashes(ptr);
        let table = set_table(ptr);
        let found = set_find_entry_with_hash(_py, order, hashes, table, key_bits, hash);
        if exception_pending(_py) {
            return;
        }
        if found.is_some() {
            return;
        }

        let new_entries = order.len() + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = set_table_capacity(new_entries);
            set_rebuild(_py, order, hashes, table, capacity);
            if exception_pending(_py) {
                return;
            }
        }

        if !reserve_set_order(_py, order, 1)
            || !reserve_hashes(_py, hashes, 1, "set allocation failed")
        {
            return;
        }
        order.push(key_bits);
        hashes.push(hash);
        inc_ref_bits(_py, key_bits);
        let entry_idx = order.len() - 1;
        set_insert_entry_with_hash(_py, order, table, entry_idx, hash);
        if crate::object::refcount_opt::is_heap_ref(key_bits) {
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
    }
}

pub(crate) unsafe fn dict_get_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
) -> Option<u64> {
    unsafe {
        // Fast path for inline integer keys: skip all exception handling,
        // hashability checks, and the heavy dict_find_entry dispatch.
        let key_obj = obj_from_bits(key_bits);
        if let Some(i) = key_obj.as_int() {
            return dict_get_inline_int_in_place(_py, ptr, key_bits, i);
        }
        // Pre-materialize the key to force NaN-box pointer resolution and
        // hash caching. This prevents Cranelift-compiled code from producing
        // stale or incorrect hash values during dict_find_entry.
        if let Some(key_ptr) = key_obj.as_ptr()
            && object_type_id(key_ptr) == TYPE_ID_STRING
        {
            let len = string_len(key_ptr);
            if len > 0 {
                std::ptr::read_volatile(string_bytes(key_ptr));
            }
        }
        if !ensure_hashable(_py, key_bits, HashContext::DictKey) {
            return None;
        }
        let pending_before = exception_pending(_py);
        let prev_exc_bits = if pending_before {
            exception_last_bits_noinc(_py).unwrap_or(0)
        } else {
            0
        };
        let order = dict_order(ptr);
        let hashes = dict_hashes(ptr);
        let table = dict_table(ptr);
        let found = dict_find_entry(_py, order, hashes, table, key_bits);
        if exception_pending(_py) {
            if !pending_before {
                return None;
            }
            let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
            if after_exc_bits != prev_exc_bits {
                return None;
            }
        }
        found.map(|idx| order[idx * 2 + 1])
    }
}

pub(crate) unsafe fn dict_find_entry_kv_in_place(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    key_bits: u64,
) -> Option<(u64, u64)> {
    unsafe {
        if !ensure_hashable(_py, key_bits, HashContext::DictKey) {
            return None;
        }
        let pending_before = exception_pending(_py);
        let prev_exc_bits = if pending_before {
            exception_last_bits_noinc(_py).unwrap_or(0)
        } else {
            0
        };
        let order = dict_order(ptr);
        let hashes = dict_hashes(ptr);
        let table = dict_table(ptr);
        let found = dict_find_entry(_py, order, hashes, table, key_bits);
        if exception_pending(_py) {
            if !pending_before {
                return None;
            }
            let after_exc_bits = exception_last_bits_noinc(_py).unwrap_or(0);
            if after_exc_bits != prev_exc_bits {
                return None;
            }
        }
        let idx = found?;
        let key_idx = idx * 2;
        Some((order[key_idx], order[key_idx + 1]))
    }
}

pub(crate) unsafe fn set_del_in_place(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64) -> bool {
    unsafe {
        // discard / remove / difference_update probe the set with the candidate
        // element; CPython reports these as a set-element insertion context on
        // 3.14 (bare on 3.12/3.13).
        if !ensure_hashable(_py, key_bits, HashContext::SetElement) {
            return false;
        }
        let order = set_order(ptr);
        let hashes = set_hashes(ptr);
        let table = set_table(ptr);
        let found = set_find_entry(_py, order, hashes, table, key_bits);
        if exception_pending(_py) {
            return false;
        }
        let Some(entry_idx) = found else {
            return false;
        };
        let key_val = order[entry_idx];
        order.remove(entry_idx);
        hashes.remove(entry_idx);
        let removed_slot_val = entry_idx + 1;
        let mut tombstones = 0usize;
        for slot in table.iter_mut() {
            if *slot == 0 {
                continue;
            }
            if *slot == TABLE_TOMBSTONE {
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot == removed_slot_val {
                *slot = TABLE_TOMBSTONE;
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot > removed_slot_val {
                *slot -= 1;
            }
        }
        let entries = order.len();
        let desired_capacity = set_table_capacity(entries.max(1));
        if table.len() > desired_capacity.saturating_mul(4)
            || tombstones.saturating_mul(4) > table.len()
        {
            set_rebuild(_py, order, hashes, table, desired_capacity);
        }
        if order.is_empty() {
            (*header_from_obj_ptr(ptr)).flags &= !crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
        dec_ref_bits(_py, key_val);
        true
    }
}

pub(crate) unsafe fn set_replace_entries(_py: &PyToken<'_>, ptr: *mut u8, entries: &[u64]) {
    unsafe {
        crate::gil_assert();
        let order = set_order(ptr);
        let hashes = set_hashes(ptr);
        let capacity = set_table_capacity(entries.len().max(1));
        if !crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            order as *mut Vec<u64>,
            entries.len(),
            "set allocation failed",
        ) {
            return;
        }
        if !crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            hashes as *mut Vec<u64>,
            entries.len(),
            "set allocation failed",
        ) {
            return;
        }
        if !crate::object::backing::tracked_vec_reserve_or_raise(
            _py,
            set_table_ptr(ptr),
            capacity,
            "set allocation failed",
        ) {
            return;
        }
        let mut replacement_hashes = Vec::with_capacity(entries.len());
        for &entry in entries {
            let hash = hash_bits(_py, entry);
            if exception_pending(_py) {
                return;
            }
            replacement_hashes.push(hash);
        }
        for entry in entries {
            inc_ref_bits(_py, *entry);
        }
        let removed: Vec<u64> = std::mem::take(order);
        hashes.clear();
        order.extend_from_slice(entries);
        hashes.extend_from_slice(&replacement_hashes);
        let table = set_table(ptr);
        set_rebuild(_py, order, hashes, table, capacity);
        if entries
            .iter()
            .any(|&entry| crate::object::refcount_opt::is_heap_ref(entry))
        {
            (*header_from_obj_ptr(ptr)).flags |= crate::object::HEADER_FLAG_CONTAINS_REFS;
        } else {
            (*header_from_obj_ptr(ptr)).flags &= !crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
        for entry in removed {
            dec_ref_bits(_py, entry);
        }
    }
}

pub(crate) unsafe fn dict_del_in_place(_py: &PyToken<'_>, ptr: *mut u8, key_bits: u64) -> bool {
    unsafe {
        if !ensure_hashable(_py, key_bits, HashContext::DictKey) {
            return false;
        }
        let order = dict_order(ptr);
        let hashes = dict_hashes(ptr);
        let table = dict_table(ptr);
        let found = dict_find_entry(_py, order, hashes, table, key_bits);
        if exception_pending(_py) {
            return false;
        }
        let Some(entry_idx) = found else {
            return false;
        };
        let key_idx = entry_idx * 2;
        let val_idx = key_idx + 1;
        let removed: Vec<u64> = order.drain(key_idx..=val_idx).collect();
        hashes.remove(entry_idx);
        let removed_slot_val = entry_idx + 1;
        let mut tombstones = 0usize;
        for slot in table.iter_mut() {
            if *slot == 0 {
                continue;
            }
            if *slot == TABLE_TOMBSTONE {
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot == removed_slot_val {
                *slot = TABLE_TOMBSTONE;
                tombstones = tombstones.saturating_add(1);
                continue;
            }
            if *slot > removed_slot_val {
                *slot -= 1;
            }
        }
        let entries = order.len() / 2;
        let desired_capacity = dict_table_capacity(entries.max(1));
        if table.len() > desired_capacity.saturating_mul(4)
            || tombstones.saturating_mul(4) > table.len()
        {
            dict_rebuild(_py, order, hashes, table, desired_capacity);
        }
        if order.is_empty() {
            (*header_from_obj_ptr(ptr)).flags &= !crate::object::HEADER_FLAG_CONTAINS_REFS;
        }
        for bits in removed {
            dec_ref_bits(_py, bits);
        }
        true
    }
}

pub(crate) unsafe fn dict_clear_in_place(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        let order = dict_order(ptr);
        let removed: Vec<u64> = std::mem::take(order);
        let hashes = dict_hashes(ptr);
        hashes.clear();
        let table = dict_table(ptr);
        table.clear();
        (*header_from_obj_ptr(ptr)).flags &= !crate::object::HEADER_FLAG_CONTAINS_REFS;
        for pair in removed.chunks_exact(2) {
            dec_ref_bits(_py, pair[0]);
            dec_ref_bits(_py, pair[1]);
        }
    }
}

pub(crate) unsafe fn dict_clear_in_place_shutdown(_py: &PyToken<'_>, ptr: *mut u8) {
    unsafe {
        crate::gil_assert();
        let order = dict_order(ptr);
        let removed: Vec<u64> = std::mem::take(order);
        let hashes = dict_hashes(ptr);
        hashes.clear();
        let table = dict_table(ptr);
        table.clear();
        (*header_from_obj_ptr(ptr)).flags &= !crate::object::HEADER_FLAG_CONTAINS_REFS;
        for pair in removed.chunks_exact(2) {
            crate::object::release_shutdown_bits(_py, pair[0]);
            crate::object::release_shutdown_bits(_py, pair[1]);
        }
    }
}
