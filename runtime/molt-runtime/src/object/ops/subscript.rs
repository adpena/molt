use super::*;

pub(crate) fn value_supports_mp_subscript(_py: &PyToken<'_>, obj_bits: u64) -> bool {
    let Some(ptr) = obj_from_bits(obj_bits).as_ptr() else {
        // Tagged immediates (int/bool/float/None) have no `tp_as_mapping`.
        return false;
    };
    let tid = unsafe { object_type_id(ptr) };
    match tid {
        TYPE_ID_DICT | TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL | TYPE_ID_TUPLE
        | TYPE_ID_STRING | TYPE_ID_BYTES | TYPE_ID_BYTEARRAY | TYPE_ID_RANGE
        | TYPE_ID_MEMORYVIEW => true,
        TYPE_ID_OBJECT => {
            // Dict subclasses and arbitrary user mappings: a real `__getitem__`
            // method resolvable on the instance's class MRO.  Mirrors the
            // `TYPE_ID_OBJECT` subscript path in `molt_index`.
            let Some(getitem_name_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") else {
                return false;
            };
            // `attr_lookup_ptr` returns `None` cleanly (no pending exception)
            // when the dunder is absent — exactly as the `TYPE_ID_OBJECT`
            // subscript path in `molt_index` relies on.
            let found = unsafe { attr_lookup_ptr(_py, ptr, getitem_name_bits) };
            dec_ref_bits(_py, getitem_name_bits);
            match found {
                Some(bound) => {
                    dec_ref_bits(_py, bound);
                    true
                }
                None => false,
            }
        }
        // Bare type objects, slice, set/frozenset, dict views, generators, …:
        // no `mp_subscript`.  (A metaclass that defines `__getitem__` so its
        // instances are subscriptable is out of scope for the class-namespace
        // contract and is not used as a `__prepare__` result in practice.)
        _ => false,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_index(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Fast path: dict[key] — skips exception_pending and type dispatch chain.
        if let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() {
            unsafe {
                if object_is_exact_builtin_dict(_py, obj_ptr) {
                    if let Some(val) = dict_get_in_place(_py, obj_ptr, key_bits) {
                        if obj_from_bits(val).as_ptr().is_some() {
                            inc_ref_bits(_py, val);
                        }
                        return val;
                    }
                    return raise_key_error_with_key(_py, key_bits);
                }
                // list_int: flat i64 storage — delegate to specialized getitem
                let tid = object_type_id(obj_ptr);
                if tid == TYPE_ID_LIST_INT {
                    return molt_list_int_getitem(obj_bits, key_bits);
                }
                // list_bool: flat u8 storage — delegate to specialized getitem
                if tid == TYPE_ID_LIST_BOOL {
                    return molt_list_bool_getitem(obj_bits, key_bits);
                }
                // tuple[int]: the most common indexed-tuple shape. Completes the
                // entry fast-path tier (dict / list_int / list_bool already have
                // one; tuple was the lone common sequence routed through the full
                // linear type-dispatch below). Only the unambiguous case is taken
                // here — exact tuple, a plain inline-int key (NOT bool/float/
                // bigint, whose index semantics CPython treats distinctly), and an
                // in-bounds offset. Every other shape (slice key, non-int key,
                // out-of-bounds, tuple subclass) falls through to the full path
                // below, so behavior is byte-identical to before this fast path.
                if tid == TYPE_ID_TUPLE {
                    let key = obj_from_bits(key_bits);
                    if key.is_int() {
                        let elems = seq_vec_ref(obj_ptr);
                        let len = elems.len() as i64;
                        let raw = key.as_int_unchecked();
                        let idx = if raw < 0 { raw + len } else { raw };
                        if idx >= 0 && idx < len {
                            let val = elems[idx as usize];
                            // inc_ref only for heap-pointer elements; inline
                            // int/float/bool/None elements carry no refcount.
                            if obj_from_bits(val).as_ptr().is_some() {
                                inc_ref_bits(_py, val);
                            }
                            return val;
                        }
                    }
                }
            }
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let obj = obj_from_bits(obj_bits);
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_MEMORYVIEW {
                    if memoryview_released(ptr) {
                        return raise_released_memoryview(_py);
                    }
                    let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                        Some(fmt) => fmt,
                        None => return MoltObject::none().bits(),
                    };
                    let data = memoryview_data(ptr);
                    if data.is_null() {
                        return MoltObject::none().bits();
                    }
                    let shape = memoryview_shape(ptr).unwrap_or(&[]);
                    let strides = memoryview_strides(ptr).unwrap_or(&[]);
                    let ndim = shape.len();
                    if ndim == 0 {
                        if let Some(tup_ptr) = key.as_ptr()
                            && object_type_id(tup_ptr) == TYPE_ID_TUPLE
                        {
                            let elems = seq_vec_ref(tup_ptr);
                            if elems.is_empty() {
                                let val = memoryview_read_scalar_at(_py, data.cast_const(), 0, fmt);
                                return val.unwrap_or_else(|| MoltObject::none().bits());
                            }
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "invalid indexing of 0-dim memory",
                        );
                    }
                    if let Some(tup_ptr) = key.as_ptr()
                        && object_type_id(tup_ptr) == TYPE_ID_TUPLE
                    {
                        let elems = seq_vec_ref(tup_ptr);
                        let mut has_slice = false;
                        let mut all_slice = true;
                        for &elem_bits in elems.iter() {
                            let elem_obj = obj_from_bits(elem_bits);
                            if let Some(elem_ptr) = elem_obj.as_ptr() {
                                if object_type_id(elem_ptr) == TYPE_ID_SLICE {
                                    has_slice = true;
                                } else {
                                    all_slice = false;
                                }
                            } else {
                                all_slice = false;
                            }
                        }
                        if has_slice {
                            if all_slice {
                                return raise_exception::<_>(
                                    _py,
                                    "NotImplementedError",
                                    "multi-dimensional slicing is not implemented",
                                );
                            }
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "memoryview: invalid slice key",
                            );
                        }
                        if elems.len() < ndim {
                            return raise_exception::<_>(
                                _py,
                                "NotImplementedError",
                                "multi-dimensional sub-views are not implemented",
                            );
                        }
                        if elems.len() > ndim {
                            let msg = format!(
                                "cannot index {}-dimension view with {}-element tuple",
                                ndim,
                                elems.len()
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        if shape.len() != strides.len() {
                            return MoltObject::none().bits();
                        }
                        let mut pos = 0isize;
                        for (dim, &elem_bits) in elems.iter().enumerate() {
                            let Some(idx) = sequence_index_i64_with_type_error(
                                _py,
                                elem_bits,
                                "memoryview: invalid slice key",
                            ) else {
                                return MoltObject::none().bits();
                            };
                            let mut i = idx;
                            let dim_len = shape[dim];
                            let dim_len_i64 = dim_len as i64;
                            if i < 0 {
                                i += dim_len_i64;
                            }
                            if i < 0 || i >= dim_len_i64 {
                                let msg = format!("index out of bounds on dimension {}", dim + 1);
                                return raise_exception::<_>(_py, "IndexError", &msg);
                            }
                            pos = pos.saturating_add((i as isize).saturating_mul(strides[dim]));
                        }
                        if pos < 0 {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "index out of bounds on dimension 1",
                            );
                        }
                        let val = memoryview_read_scalar_at(_py, data.cast_const(), pos, fmt);
                        return val.unwrap_or_else(|| MoltObject::none().bits());
                    }
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let len = shape[0];
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let base_offset = memoryview_offset(ptr);
                        let base_stride = strides[0];
                        let itemsize = memoryview_itemsize(ptr);
                        let new_offset = base_offset + start * base_stride;
                        let new_stride = base_stride * step;
                        let new_len = range_len_i64(start as i64, stop as i64, step as i64);
                        let new_len = new_len.max(0) as usize;
                        let mut new_shape = shape.to_vec();
                        let mut new_strides = strides.to_vec();
                        if !new_shape.is_empty() {
                            new_shape[0] = new_len as isize;
                            new_strides[0] = new_stride;
                        }
                        let storage = TypedStridedStorage::new(
                            data.offset(start.saturating_mul(base_stride)),
                            memoryview_readonly(ptr),
                            itemsize,
                            new_offset,
                            memoryview_base_bits(ptr),
                            memoryview_format_bits(ptr),
                            new_shape,
                            new_strides,
                        );
                        let out_ptr = match storage {
                            Some(storage) => alloc_memoryview_from_storage(_py, storage),
                            None => std::ptr::null_mut(),
                        };
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    if ndim > 1 {
                        return raise_exception::<_>(
                            _py,
                            "NotImplementedError",
                            "multi-dimensional sub-views are not implemented",
                        );
                    }
                    let Some(idx) = sequence_index_i64_with_type_error(
                        _py,
                        key_bits,
                        "memoryview: invalid slice key",
                    ) else {
                        return MoltObject::none().bits();
                    };
                    let len = shape[0] as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "index out of bounds on dimension 1",
                        );
                    }
                    let pos = (i as isize) * strides[0];
                    if pos < 0 {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "index out of bounds on dimension 1",
                        );
                    }
                    let val = memoryview_read_scalar_at(_py, data.cast_const(), pos, fmt);
                    return val.unwrap_or_else(|| MoltObject::none().bits());
                }
                if type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let bytes = if type_id == TYPE_ID_STRING {
                            std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr))
                        } else {
                            std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr))
                        };
                        let len = if type_id == TYPE_ID_STRING {
                            utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize)) as isize
                        } else {
                            bytes.len() as isize
                        };
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let out_ptr = if step == 1 {
                            let s = start as usize;
                            let e = stop as usize;
                            if s >= e {
                                if type_id == TYPE_ID_STRING {
                                    alloc_string(_py, &[])
                                } else if type_id == TYPE_ID_BYTES {
                                    alloc_bytes(_py, &[])
                                } else {
                                    alloc_bytearray(_py, &[])
                                }
                            } else if type_id == TYPE_ID_STRING {
                                let start_byte = utf8_char_to_byte_index_cached(
                                    _py,
                                    bytes,
                                    s as i64,
                                    Some(ptr as usize),
                                );
                                let end_byte = utf8_char_to_byte_index_cached(
                                    _py,
                                    bytes,
                                    e as i64,
                                    Some(ptr as usize),
                                );
                                alloc_string(_py, &bytes[start_byte..end_byte])
                            } else if type_id == TYPE_ID_BYTES {
                                alloc_bytes(_py, &bytes[s..e])
                            } else {
                                alloc_bytearray(_py, &bytes[s..e])
                            }
                        } else {
                            let indices = collect_slice_indices(start, stop, step);
                            let mut out = Vec::with_capacity(indices.len());
                            if type_id == TYPE_ID_STRING {
                                for idx in indices {
                                    if let Some(code) = wtf8_codepoint_at(bytes, idx) {
                                        push_wtf8_codepoint(&mut out, code.to_u32());
                                    }
                                }
                            } else {
                                for idx in indices {
                                    out.push(bytes[idx]);
                                }
                            }
                            if type_id == TYPE_ID_STRING {
                                alloc_string(_py, &out)
                            } else if type_id == TYPE_ID_BYTES {
                                alloc_bytes(_py, &out)
                            } else {
                                alloc_bytearray(_py, &out)
                            }
                        };
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    let idx = if type_id == TYPE_ID_BYTEARRAY {
                        sequence_index_i64(_py, key_bits, "bytearray")
                    } else {
                        let type_err = if type_id == TYPE_ID_STRING {
                            format!(
                                "string indices must be integers, not '{}'",
                                type_name(_py, key)
                            )
                        } else {
                            format!(
                                "byte indices must be integers or slices, not {}",
                                type_name(_py, key)
                            )
                        };
                        sequence_index_i64_with_type_error(_py, key_bits, &type_err)
                    };
                    let Some(idx) = idx else {
                        return MoltObject::none().bits();
                    };
                    if type_id == TYPE_ID_STRING {
                        let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                        let mut i = idx;
                        let len = utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize));
                        if i < 0 {
                            i += len;
                        }
                        if i < 0 || i >= len {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "string index out of range",
                            );
                        }
                        let Some(code) = wtf8_codepoint_at(bytes, i as usize) else {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "string index out of range",
                            );
                        };
                        let mut out = Vec::with_capacity(4);
                        push_wtf8_codepoint(&mut out, code.to_u32());
                        let out_ptr = alloc_string(_py, &out);
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    let bytes = std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr));
                    let len = bytes.len() as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        if type_id == TYPE_ID_BYTEARRAY {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "bytearray index out of range",
                            );
                        }
                        return raise_exception::<_>(_py, "IndexError", "index out of range");
                    }
                    return MoltObject::from_int(bytes[i as usize] as i64).bits();
                }
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_LIST {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let elems = seq_vec_ref(ptr);
                        let len = elems.len() as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let out_ptr = if step == 1 {
                            let s = start as usize;
                            let e = stop as usize;
                            if s >= e {
                                alloc_list(_py, &[])
                            } else {
                                alloc_list(_py, &elems[s..e])
                            }
                        } else {
                            let indices = collect_slice_indices(start, stop, step);
                            let mut out = Vec::with_capacity(indices.len());
                            for idx in indices {
                                out.push(elems[idx]);
                            }
                            alloc_list(_py, out.as_slice())
                        };
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    // CPython sequence subscript requires the integer protocol
                    // (`__index__`): int / bool / int-subclass / object with
                    // `__index__`. A float — even an integral `2.0` — has no
                    // `nb_index` and must raise TypeError, never be truncated.
                    // `sequence_index_i64` is the single authority enforcing this;
                    // never reintroduce `to_i64` here (it accepts integral floats
                    // and silently diverges from CPython).
                    let Some(idx) = sequence_index_i64(_py, key_bits, "list") else {
                        return MoltObject::none().bits();
                    };
                    let len = list_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        if debug_index_enabled() {
                            let task = crate::current_task_key()
                                .map(|slot| slot.0 as usize)
                                .unwrap_or(0);
                            eprintln!(
                                "molt index oob task=0x{:x} type=list len={} idx={}",
                                task, len, i
                            );
                        }
                        return raise_exception::<_>(_py, "IndexError", "list index out of range");
                    }
                    let elems = seq_vec_ref(ptr);
                    let val = elems[i as usize];
                    if debug_index_list_enabled() {
                        let val_obj = obj_from_bits(val);
                        eprintln!(
                            "molt_index list obj=0x{:x} idx={} val_type={} val_bits=0x{:x}",
                            obj_bits,
                            i,
                            type_name(_py, val_obj),
                            val
                        );
                    }
                    inc_ref_bits(_py, val);
                    return val;
                }
                if type_id == TYPE_ID_TUPLE {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let elems = seq_vec_ref(ptr);
                        let len = elems.len() as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let out_ptr = if step == 1 {
                            let s = start as usize;
                            let e = stop as usize;
                            if s >= e {
                                alloc_tuple(_py, &[])
                            } else {
                                alloc_tuple(_py, &elems[s..e])
                            }
                        } else {
                            let indices = collect_slice_indices(start, stop, step);
                            let mut out = Vec::with_capacity(indices.len());
                            for idx in indices {
                                out.push(elems[idx]);
                            }
                            alloc_tuple(_py, out.as_slice())
                        };
                        if out_ptr.is_null() {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_ptr(out_ptr).bits();
                    }
                    // `__index__`-only key coercion (see the list branch above):
                    // float keys raise TypeError, they are not truncated.
                    let Some(idx) = sequence_index_i64(_py, key_bits, "tuple") else {
                        return MoltObject::none().bits();
                    };
                    let len = tuple_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        if debug_index_enabled() {
                            let task = crate::current_task_key()
                                .map(|slot| slot.0 as usize)
                                .unwrap_or(0);
                            eprintln!(
                                "molt index oob task=0x{:x} type=tuple len={} idx={}",
                                task, len, i
                            );
                        }
                        return raise_exception::<_>(_py, "IndexError", "tuple index out of range");
                    }
                    let elems = seq_vec_ref(ptr);
                    let val = elems[i as usize];
                    inc_ref_bits(_py, val);
                    return val;
                }
                if type_id == TYPE_ID_RANGE {
                    // `__index__`-only key coercion: `index_i64_integral_bits`
                    // accepts int / bool / int-subclass but rejects float, so a
                    // float key falls through to the bigint fallback below, which
                    // raises the standard TypeError. A bare bigint / `__index__`
                    // object also routes to the fallback (correct, just colder).
                    if let Some((start_i64, stop_i64, step_i64)) = range_components_i64(ptr)
                        && let Some(mut idx_i64) = index_i64_integral_bits(key_bits)
                    {
                        if idx_i64 < 0 {
                            let len = range_len_i128(start_i64, stop_i64, step_i64);
                            let adj = (idx_i64 as i128) + len;
                            if adj < 0 {
                                return raise_exception::<_>(
                                    _py,
                                    "IndexError",
                                    "range object index out of range",
                                );
                            }
                            idx_i64 = match i64::try_from(adj) {
                                Ok(v) => v,
                                Err(_) => {
                                    return raise_exception::<_>(
                                        _py,
                                        "IndexError",
                                        "range object index out of range",
                                    );
                                }
                            };
                        }
                        if let Some(value) =
                            range_value_at_index_i64(start_i64, stop_i64, step_i64, idx_i64 as i128)
                        {
                            return MoltObject::from_int(value).bits();
                        }
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "range object index out of range",
                        );
                    }
                    let Some(mut idx) = sequence_index_bigint(_py, key_bits, "range") else {
                        return MoltObject::none().bits();
                    };
                    let Some((start, stop, step)) = range_components_bigint(ptr) else {
                        return MoltObject::none().bits();
                    };
                    let len = range_len_bigint(&start, &stop, &step);
                    if idx.is_negative() {
                        idx += &len;
                    }
                    if idx.is_negative() || idx >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "range object index out of range",
                        );
                    }
                    let val = start + step * idx;
                    return int_bits_from_bigint(_py, val);
                }
                if type_id != TYPE_ID_DICT {
                    let class_bits = object_class_bits(ptr);
                    if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
                        && object_type_id(class_ptr) == TYPE_ID_TYPE
                        && let Some(getitem_name_bits) =
                            attr_name_bits_from_bytes(_py, b"__getitem__")
                    {
                        let explicit_getitem = obj_from_bits(class_dict_bits(class_ptr))
                            .as_ptr()
                            .is_some_and(|dict_ptr| {
                                object_type_id(dict_ptr) == TYPE_ID_DICT
                                    && dict_get_in_place(_py, dict_ptr, getitem_name_bits).is_some()
                            });
                        if explicit_getitem {
                            if let Some(call_bits) = class_attr_lookup(
                                _py,
                                class_ptr,
                                class_ptr,
                                Some(ptr),
                                getitem_name_bits,
                            ) {
                                dec_ref_bits(_py, getitem_name_bits);
                                exception_stack_push();
                                let res = call_callable1(_py, call_bits, key_bits);
                                dec_ref_bits(_py, call_bits);
                                if exception_pending(_py) {
                                    exception_stack_pop(_py);
                                    return MoltObject::none().bits();
                                }
                                exception_stack_pop(_py);
                                return res;
                            }
                            if exception_pending(_py) {
                                dec_ref_bits(_py, getitem_name_bits);
                                return MoltObject::none().bits();
                            }
                        }
                        dec_ref_bits(_py, getitem_name_bits);
                    }
                }
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if let Some(val) = dict_get_in_place(_py, dict_ptr, key_bits) {
                        // Skip inc_ref for inline values (ints, bools, None).
                        if obj_from_bits(val).as_ptr().is_some() {
                            inc_ref_bits(_py, val);
                        }
                        return val;
                    }
                    if !object_is_exact_builtin_dict(_py, ptr)
                        && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__missing__")
                    {
                        if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits)
                        {
                            dec_ref_bits(_py, name_bits);
                            exception_stack_push();
                            let res = call_callable1(_py, call_bits, key_bits);
                            dec_ref_bits(_py, call_bits);
                            if exception_pending(_py) {
                                exception_stack_pop(_py);
                                return MoltObject::none().bits();
                            }
                            exception_stack_pop(_py);
                            return res;
                        }
                        dec_ref_bits(_py, name_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    return raise_key_error_with_key(_py, key_bits);
                }
                if type_id == TYPE_ID_DICT_KEYS_VIEW
                    || type_id == TYPE_ID_DICT_VALUES_VIEW
                    || type_id == TYPE_ID_DICT_ITEMS_VIEW
                {
                    let view_name = type_name(_py, obj);
                    let msg = format!("'{}' object is not subscriptable", view_name);
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if type_id == TYPE_ID_TYPE {
                    // Try explicit __class_getitem__ first (handles custom
                    // implementations in user-defined classes).
                    if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__class_getitem__") {
                        if let Some(call_bits) =
                            class_attr_lookup(_py, ptr, ptr, Some(ptr), name_bits)
                        {
                            dec_ref_bits(_py, name_bits);
                            exception_stack_push();
                            let res = call_callable1(_py, call_bits, key_bits);
                            dec_ref_bits(_py, call_bits);
                            if exception_pending(_py) {
                                exception_stack_pop(_py);
                                return MoltObject::none().bits();
                            }
                            exception_stack_pop(_py);
                            return res;
                        }
                        dec_ref_bits(_py, name_bits);
                    }
                    // CPython rule: a type is subscriptable IFF `__class_getitem__`
                    // is resolvable on it. The explicit-call path above handles a
                    // *bindable* `__class_getitem__` (user classes with a plain
                    // `def __class_getitem__`). The remaining case is the DEFAULT
                    // `__class_getitem__ = classmethod(GenericAlias)` carried by the
                    // generic-capable builtins, `collections.abc.*`, `typing.*`, and
                    // PEP 695 generics — whose bound form is a classmethod wrapping
                    // the `GenericAlias` *type* (not a function) that the call path
                    // intentionally does not invoke. For those we PRESENCE-check
                    // `__class_getitem__` in the MRO (no bind/call) and, when found,
                    // produce the default `GenericAlias(cls, params)` directly — the
                    // exact value `classmethod(GenericAlias)(cls, params)` yields.
                    //
                    // When `__class_getitem__` is ABSENT from the entire MRO
                    // (`int`, `str`, `float`, `bool`, `bytes`, `complex`, `object`,
                    // `range`, `slice`, `bytearray`, a bare user `class C: ...`),
                    // the type is NOT subscriptable: fall through to the shared
                    // not-subscriptable raise, which emits
                    // `TypeError: type 'X' is not subscriptable` for a
                    // `TYPE_ID_TYPE` receiver — byte-identical to CPython
                    // 3.12/3.13/3.14. This removes molt's prior unconditional
                    // default-GenericAlias-for-every-type divergence.
                    if let Some(cgi_name_bits) =
                        attr_name_bits_from_bytes(_py, b"__class_getitem__")
                    {
                        let present = class_attr_lookup_raw_mro(_py, ptr, cgi_name_bits).is_some();
                        dec_ref_bits(_py, cgi_name_bits);
                        if present {
                            return crate::builtins::types::molt_generic_alias_new(
                                obj_bits, key_bits,
                            );
                        }
                    }
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") {
                    if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        exception_stack_push();
                        let res = call_callable1(_py, call_bits, key_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            exception_stack_pop(_py);
                            return MoltObject::none().bits();
                        }
                        exception_stack_pop(_py);
                        return res;
                    }
                    dec_ref_bits(_py, name_bits);
                }
            }
            let msg = if unsafe { object_type_id(ptr) } == TYPE_ID_TYPE {
                let class_name =
                    unsafe { string_obj_to_owned(obj_from_bits(class_name_bits(ptr))) }
                        .unwrap_or_else(|| "object".to_string());
                if debug_subscript_enabled() {
                    eprintln!(
                        "[MOLT-DEBUG] subscript fail (TYPE_ID_TYPE, no __class_getitem__): class_name={}, obj_bits=0x{:016x}, key_bits=0x{:016x}",
                        class_name, obj_bits, key_bits
                    );
                }
                format!("type '{}' is not subscriptable", class_name)
            } else {
                let tn = type_name(_py, obj);
                let tid = unsafe { object_type_id(ptr) };
                if debug_subscript_enabled() {
                    eprintln!(
                        "[MOLT-DEBUG] subscript fail (ptr path): type_name={}, type_id={}, obj_bits=0x{:016x}, key_bits=0x{:016x}",
                        tn, tid, obj_bits, key_bits
                    );
                }
                format!("'{}' object is not subscriptable", tn)
            };
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let obj_dbg = obj_from_bits(obj_bits);
        if debug_subscript_enabled() {
            eprintln!(
                "[MOLT-DEBUG] subscript fail (no-ptr path): type_name={}, obj_bits=0x{:016x}, key_bits=0x{:016x}, is_int={}, is_float={}, is_bool={}, is_none={}, is_pending={}",
                type_name(_py, obj_dbg),
                obj_bits,
                key_bits,
                obj_dbg.is_int(),
                obj_dbg.is_float(),
                obj_dbg.is_bool(),
                obj_dbg.is_none(),
                obj_dbg.is_pending()
            );
        }
        let msg = format!("'{}' object is not subscriptable", type_name(_py, obj));
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ord_at(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_STRING {
                    if key
                        .as_ptr()
                        .is_some_and(|key_ptr| object_type_id(key_ptr) == TYPE_ID_SLICE)
                    {
                        let indexed = molt_index(obj_bits, key_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        let out = crate::object::ops_sys::molt_ord(indexed);
                        if obj_from_bits(indexed).as_ptr().is_some() {
                            dec_ref_bits(_py, indexed);
                        }
                        return out;
                    }
                    let type_err = format!(
                        "string indices must be integers, not '{}'",
                        type_name(_py, key)
                    );
                    let Some(idx) = sequence_index_i64_with_type_error(_py, key_bits, &type_err)
                    else {
                        return MoltObject::none().bits();
                    };
                    let bytes = std::slice::from_raw_parts(string_bytes(ptr), string_len(ptr));
                    let len = utf8_codepoint_count_cached(_py, bytes, Some(ptr as usize));
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "string index out of range",
                        );
                    }
                    let Some(code) = wtf8_codepoint_at(bytes, i as usize) else {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "string index out of range",
                        );
                    };
                    return MoltObject::from_int(code.to_u32() as i64).bits();
                }
            }
        }
        let indexed = molt_index(obj_bits, key_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let out = crate::object::ops_sys::molt_ord(indexed);
        if obj_from_bits(indexed).as_ptr().is_some() {
            dec_ref_bits(_py, indexed);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_store_index(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        // Fast path: dict[key] = val — skips type dispatch chain.
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_DICT {
                    dict_set_in_place(_py, ptr, key_bits, val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return obj_bits;
                }
                // list_int: flat i64 storage — delegate to specialized setitem
                let tid = object_type_id(ptr);
                if tid == TYPE_ID_LIST_INT {
                    return molt_list_int_setitem(obj_bits, key_bits, val_bits);
                }
                // list_bool: flat u8 storage — delegate to specialized setitem
                if tid == TYPE_ID_LIST_BOOL {
                    return molt_list_bool_setitem(obj_bits, key_bits, val_bits);
                }
            }
        }
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_LIST_BOOL
                    || object_type_id(ptr) == TYPE_ID_LIST_INT
                {
                    crate::object::ops_list::promote_specialized_list_to_list(_py, ptr);
                }
                let type_id = object_type_id(ptr);
                if type_id == TYPE_ID_LIST {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let len = list_len(ptr) as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let new_items = match collect_iterable_values(
                            _py,
                            val_bits,
                            "must assign iterable to extended slice",
                        ) {
                            Some(items) => items,
                            None => return MoltObject::none().bits(),
                        };
                        let elems = seq_vec(ptr);
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            let new_items_have_refs = new_items
                                .iter()
                                .any(|&item| crate::object::refcount_opt::is_heap_ref(item));
                            for &item in new_items.iter() {
                                if crate::object::refcount_opt::is_heap_ref(item) {
                                    inc_ref_bits(_py, item);
                                }
                            }
                            let removed: Vec<u64> =
                                elems.splice(s..e, new_items.iter().copied()).collect();
                            if new_items_have_refs {
                                (*header_from_obj_ptr(ptr)).flags |=
                                    crate::object::HEADER_FLAG_CONTAINS_REFS;
                            }
                            for old_bits in removed {
                                if crate::object::refcount_opt::is_heap_ref(old_bits) {
                                    dec_ref_bits(_py, old_bits);
                                }
                            }
                            return obj_bits;
                        }
                        let indices = collect_slice_indices(start, stop, step);
                        if indices.len() != new_items.len() {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                &format!(
                                    "attempt to assign sequence of size {} to extended slice of size {}",
                                    new_items.len(),
                                    indices.len()
                                ),
                            );
                        }
                        let new_items_have_refs = new_items
                            .iter()
                            .any(|&item| crate::object::refcount_opt::is_heap_ref(item));
                        for &item in new_items.iter() {
                            if crate::object::refcount_opt::is_heap_ref(item) {
                                inc_ref_bits(_py, item);
                            }
                        }
                        let mut removed = Vec::new();
                        for (idx, &item) in indices.iter().zip(new_items.iter()) {
                            let old_bits = elems[*idx];
                            if old_bits != item {
                                elems[*idx] = item;
                                removed.push(old_bits);
                            }
                        }
                        if new_items_have_refs {
                            (*header_from_obj_ptr(ptr)).flags |=
                                crate::object::HEADER_FLAG_CONTAINS_REFS;
                        }
                        for old_bits in removed {
                            if crate::object::refcount_opt::is_heap_ref(old_bits) {
                                dec_ref_bits(_py, old_bits);
                            }
                        }
                        return obj_bits;
                    }
                    // `__index__`-only key coercion (see `molt_index`): assigning
                    // through a float key (`L[2.0] = x`) raises TypeError.
                    let Some(idx) = sequence_index_i64(_py, key_bits, "list") else {
                        return MoltObject::none().bits();
                    };
                    if debug_store_index_enabled() {
                        let val_obj = obj_from_bits(val_bits);
                        eprintln!(
                            "molt_store_index list obj=0x{:x} idx={} val_type={} val_bits=0x{:x}",
                            obj_bits,
                            idx,
                            type_name(_py, val_obj),
                            val_bits
                        );
                    }
                    let len = list_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "list assignment index out of range",
                        );
                    }
                    let elems = seq_vec(ptr);
                    let old_bits = elems[i as usize];
                    if old_bits != val_bits {
                        if crate::object::refcount_opt::is_heap_ref(val_bits) {
                            inc_ref_bits(_py, val_bits);
                            (*header_from_obj_ptr(ptr)).flags |=
                                crate::object::HEADER_FLAG_CONTAINS_REFS;
                        }
                        elems[i as usize] = val_bits;
                        if crate::object::refcount_opt::is_heap_ref(old_bits) {
                            dec_ref_bits(_py, old_bits);
                        }
                    }
                    return obj_bits;
                }
                if type_id == TYPE_ID_TUPLE {
                    // CPython: `t[i] = x` / `t[i:j] = ...` raise TypeError via the
                    // missing sq_ass_item slot. Previously a silent no-op (data
                    // unmodified, no error) — a divergence. Version-stable
                    // message across 3.12/3.13/3.14 for both index and slice.
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "'tuple' object does not support item assignment",
                    );
                }
                if type_id == TYPE_ID_RANGE {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "'range' object does not support item assignment",
                    );
                }
                // Immutable / non-subscript-assignable builtins: `s[i] = x`,
                // `s[i:j] = ...`. CPython raises TypeError via the missing
                // sq_ass_item / mp_ass_subscript slot. Previously these fell all
                // the way through to the silent `none` no-op below (data
                // unmodified, no error) — a P0 silent-miscompile (e.g. #52:
                // `s = "hello"; s[0] = "H"` succeeded). The message is
                // `'<type>' object does not support item assignment` for every
                // such type and is version-stable across 3.12/3.13/3.14 for both
                // the index and slice forms.
                let immutable_assign_type_name = match type_id {
                    TYPE_ID_STRING => Some("str"),
                    TYPE_ID_BYTES => Some("bytes"),
                    TYPE_ID_SET => Some("set"),
                    TYPE_ID_FROZENSET => Some("frozenset"),
                    _ => None,
                };
                if let Some(name) = immutable_assign_type_name {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!("'{}' object does not support item assignment", name),
                    );
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let len = bytes_len(ptr) as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let src_bytes = match collect_bytearray_assign_bytes(_py, val_bits) {
                            Some(bytes) => bytes,
                            None => return MoltObject::none().bits(),
                        };
                        let elems = bytearray_vec(ptr);
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            elems.splice(s..e, src_bytes.iter().copied());
                            return obj_bits;
                        }
                        let indices = collect_slice_indices(start, stop, step);
                        if indices.len() != src_bytes.len() {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                &format!(
                                    "attempt to assign bytes of size {} to extended slice of size {}",
                                    src_bytes.len(),
                                    indices.len()
                                ),
                            );
                        }
                        for (idx, byte) in indices.iter().zip(src_bytes.iter()) {
                            elems[*idx] = *byte;
                        }
                        return obj_bits;
                    }
                    // `__index__`-only key coercion (see `molt_index`): a float
                    // key raises TypeError, it is not truncated.
                    let Some(idx) = sequence_index_i64(_py, key_bits, "bytearray") else {
                        return MoltObject::none().bits();
                    };
                    let len = bytes_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "bytearray index out of range",
                        );
                    }
                    let Some(byte) = bytes_item_to_u8(_py, val_bits, BytesCtorKind::Bytearray)
                    else {
                        return MoltObject::none().bits();
                    };
                    let elems = bytearray_vec(ptr);
                    elems[i as usize] = byte;
                    return obj_bits;
                }
                if type_id == TYPE_ID_MEMORYVIEW {
                    if memoryview_released(ptr) {
                        return raise_released_memoryview(_py);
                    }
                    if memoryview_readonly(ptr) {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "cannot modify read-only memory",
                        );
                    }
                    let data = memoryview_data(ptr);
                    if data.is_null() {
                        return MoltObject::none().bits();
                    }
                    let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                        Some(fmt) => fmt,
                        None => return MoltObject::none().bits(),
                    };
                    let shape = memoryview_shape(ptr).unwrap_or(&[]);
                    let strides = memoryview_strides(ptr).unwrap_or(&[]);
                    let ndim = shape.len();
                    if ndim == 0 {
                        if let Some(tup_ptr) = key.as_ptr()
                            && object_type_id(tup_ptr) == TYPE_ID_TUPLE
                        {
                            let elems = seq_vec_ref(tup_ptr);
                            if elems.is_empty() {
                                let ok = memoryview_write_scalar_at(_py, data, 0, fmt, val_bits);
                                if ok.is_none() {
                                    return MoltObject::none().bits();
                                }
                                return obj_bits;
                            }
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "invalid indexing of 0-dim memory",
                        );
                    }
                    if let Some(tup_ptr) = key.as_ptr()
                        && object_type_id(tup_ptr) == TYPE_ID_TUPLE
                    {
                        let elems = seq_vec_ref(tup_ptr);
                        let mut has_slice = false;
                        let mut all_slice = true;
                        for &elem_bits in elems.iter() {
                            let elem_obj = obj_from_bits(elem_bits);
                            if let Some(elem_ptr) = elem_obj.as_ptr() {
                                if object_type_id(elem_ptr) == TYPE_ID_SLICE {
                                    has_slice = true;
                                } else {
                                    all_slice = false;
                                }
                            } else {
                                all_slice = false;
                            }
                        }
                        if has_slice {
                            if all_slice {
                                return raise_exception::<_>(
                                    _py,
                                    "NotImplementedError",
                                    "memoryview slice assignments are currently restricted to ndim = 1",
                                );
                            }
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "memoryview: invalid slice key",
                            );
                        }
                        if elems.len() < ndim {
                            return raise_exception::<_>(
                                _py,
                                "NotImplementedError",
                                "sub-views are not implemented",
                            );
                        }
                        if elems.len() > ndim {
                            let msg = format!(
                                "cannot index {}-dimension view with {}-element tuple",
                                ndim,
                                elems.len()
                            );
                            return raise_exception::<_>(_py, "TypeError", &msg);
                        }
                        if shape.len() != strides.len() {
                            return MoltObject::none().bits();
                        }
                        let mut pos = 0isize;
                        for (dim, &elem_bits) in elems.iter().enumerate() {
                            let Some(idx) = sequence_index_i64_with_type_error(
                                _py,
                                elem_bits,
                                "memoryview: invalid slice key",
                            ) else {
                                return MoltObject::none().bits();
                            };
                            let mut i = idx;
                            let dim_len = shape[dim];
                            let dim_len_i64 = dim_len as i64;
                            if i < 0 {
                                i += dim_len_i64;
                            }
                            if i < 0 || i >= dim_len_i64 {
                                let msg = format!("index out of bounds on dimension {}", dim + 1);
                                return raise_exception::<_>(_py, "IndexError", &msg);
                            }
                            pos = pos.saturating_add((i as isize).saturating_mul(strides[dim]));
                        }
                        if pos < 0 {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "index out of bounds on dimension 1",
                            );
                        }
                        let ok = memoryview_write_scalar_at(_py, data, pos, fmt, val_bits);
                        if ok.is_none() {
                            return MoltObject::none().bits();
                        }
                        return obj_bits;
                    }
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        if ndim != 1 {
                            return raise_exception::<_>(
                                _py,
                                "NotImplementedError",
                                "memoryview slice assignments are currently restricted to ndim = 1",
                            );
                        }
                        let len = shape[0];
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let indices = collect_slice_indices(start, stop, step);
                        let elem_count = indices.len();
                        let val_obj = obj_from_bits(val_bits);
                        let src_bytes = if let Some(src_ptr) = val_obj.as_ptr() {
                            let src_type = object_type_id(src_ptr);
                            if src_type == TYPE_ID_BYTES || src_type == TYPE_ID_BYTEARRAY {
                                if fmt.code != b'B' {
                                    return raise_exception::<_>(
                                        _py,
                                        "ValueError",
                                        "memoryview assignment: lvalue and rvalue have different structures",
                                    );
                                }
                                bytes_like_slice_raw(src_ptr).unwrap_or(&[]).to_vec()
                            } else if src_type == TYPE_ID_MEMORYVIEW {
                                if memoryview_released(src_ptr) {
                                    return raise_released_memoryview(_py);
                                }
                                let src_fmt = match memoryview_format_from_bits(
                                    memoryview_format_bits(src_ptr),
                                ) {
                                    Some(fmt) => fmt,
                                    None => return MoltObject::none().bits(),
                                };
                                let src_shape = memoryview_shape(src_ptr).unwrap_or(&[]);
                                if src_fmt.code != fmt.code
                                    || src_shape.len() != 1
                                    || src_shape[0] as usize != elem_count
                                {
                                    return raise_exception::<_>(
                                        _py,
                                        "ValueError",
                                        "memoryview assignment: lvalue and rvalue have different structures",
                                    );
                                }
                                match memoryview_collect_bytes(src_ptr) {
                                    Some(buf) => buf,
                                    None => return MoltObject::none().bits(),
                                }
                            } else {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    &format!(
                                        "a bytes-like object is required, not '{}'",
                                        type_name(_py, val_obj)
                                    ),
                                );
                            }
                        } else {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                &format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, val_obj)
                                ),
                            );
                        };
                        let expected = elem_count * fmt.itemsize;
                        if src_bytes.len() != expected {
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                "memoryview assignment: lvalue and rvalue have different structures",
                            );
                        }
                        let base_offset = memoryview_offset(ptr);
                        let base_stride = strides[0];
                        let mut pos = base_offset + start * base_stride;
                        let step_stride = base_stride * step;
                        let mut idx = 0usize;
                        while idx < src_bytes.len() {
                            if pos < 0 {
                                return MoltObject::none().bits();
                            }
                            let dst =
                                std::slice::from_raw_parts_mut(data.offset(pos), fmt.itemsize);
                            dst.copy_from_slice(&src_bytes[idx..idx + fmt.itemsize]);
                            idx += fmt.itemsize;
                            pos += step_stride;
                        }
                        return obj_bits;
                    }
                    if ndim != 1 {
                        return raise_exception::<_>(
                            _py,
                            "NotImplementedError",
                            "sub-views are not implemented",
                        );
                    }
                    let Some(idx) = sequence_index_i64_with_type_error(
                        _py,
                        key_bits,
                        "memoryview: invalid slice key",
                    ) else {
                        return MoltObject::none().bits();
                    };
                    let len = shape[0] as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "index out of bounds on dimension 1",
                        );
                    }
                    let pos = (i as isize) * strides[0];
                    if pos < 0 {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "index out of bounds on dimension 1",
                        );
                    }
                    let ok = memoryview_write_scalar_at(_py, data, pos, fmt, val_bits);
                    if ok.is_none() {
                        return MoltObject::none().bits();
                    }
                    return obj_bits;
                }
                if type_id == TYPE_ID_OBJECT {
                    let class_bits = object_class_bits(ptr);
                    if class_bits != 0 {
                        let mappingproxy_bits =
                            crate::builtins::types::mappingproxy_class_bits(_py);
                        if class_bits == mappingproxy_bits {
                            return raise_exception::<u64>(
                                _py,
                                "TypeError",
                                "'mappingproxy' object does not support item assignment",
                            );
                        }
                        let builtins = builtin_classes(_py);
                        if issubclass_bits(class_bits, builtins.dict)
                            && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__setitem__")
                        {
                            if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                                dec_ref_bits(_py, name_bits);
                                exception_stack_push();
                                let _ = call_callable2(_py, call_bits, key_bits, val_bits);
                                dec_ref_bits(_py, call_bits);
                                if exception_pending(_py) {
                                    exception_stack_pop(_py);
                                    return MoltObject::none().bits();
                                }
                                exception_stack_pop(_py);
                                return obj_bits;
                            }
                            dec_ref_bits(_py, name_bits);
                        }
                    }
                }
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return obj_bits;
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__setitem__") {
                    if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        exception_stack_push();
                        let _ = call_callable2(_py, call_bits, key_bits, val_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            exception_stack_pop(_py);
                            return MoltObject::none().bits();
                        }
                        exception_stack_pop(_py);
                        return obj_bits;
                    }
                    dec_ref_bits(_py, name_bits);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_del_index(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if object_type_id(ptr) == TYPE_ID_LIST_BOOL
                    || object_type_id(ptr) == TYPE_ID_LIST_INT
                {
                    crate::object::ops_list::promote_specialized_list_to_list(_py, ptr);
                }
                let type_id = object_type_id(ptr);
                // CPython: `del t[i]` / `del t[i:j]` raise TypeError for every
                // immutable / non-subscript-deletable builtin. Previously these
                // fell through to the silent `none` no-op below (no error) — the
                // deletion twin of the #52 store-index silent-miscompile. Wording
                // asymmetry CPython applies uniformly, version-stable on
                // 3.12/3.13/3.14: index deletion (sq_ass_item slot) says
                // "doesn't support item deletion"; slice deletion (the
                // subscript-del path) says "does not support item deletion".
                let immutable_del_type_name = match type_id {
                    TYPE_ID_TUPLE => Some("tuple"),
                    TYPE_ID_RANGE => Some("range"),
                    TYPE_ID_STRING => Some("str"),
                    TYPE_ID_BYTES => Some("bytes"),
                    TYPE_ID_SET => Some("set"),
                    TYPE_ID_FROZENSET => Some("frozenset"),
                    _ => None,
                };
                if let Some(type_name) = immutable_del_type_name {
                    let is_slice = key
                        .as_ptr()
                        .is_some_and(|p| object_type_id(p) == TYPE_ID_SLICE);
                    let msg = if is_slice {
                        format!("'{type_name}' object does not support item deletion")
                    } else {
                        format!("'{type_name}' object doesn't support item deletion")
                    };
                    return raise_exception::<_>(_py, "TypeError", &msg);
                }
                if type_id == TYPE_ID_LIST {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let len = list_len(ptr) as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let elems = seq_vec(ptr);
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            let removed: Vec<u64> = elems.drain(s..e).collect();
                            for old_bits in removed {
                                dec_ref_bits(_py, old_bits);
                            }
                            return obj_bits;
                        }
                        let indices = collect_slice_indices(start, stop, step);
                        let mut removed = Vec::with_capacity(indices.len());
                        if step > 0 {
                            for &idx in indices.iter().rev() {
                                let old_bits = elems.remove(idx);
                                removed.push(old_bits);
                            }
                        } else {
                            for &idx in indices.iter() {
                                let old_bits = elems.remove(idx);
                                removed.push(old_bits);
                            }
                        }
                        for old_bits in removed {
                            dec_ref_bits(_py, old_bits);
                        }
                        return obj_bits;
                    }
                    // `__index__`-only key coercion (see `molt_index`): deleting
                    // through a float key (`del L[2.0]`) raises TypeError.
                    let Some(idx) = sequence_index_i64(_py, key_bits, "list") else {
                        return MoltObject::none().bits();
                    };
                    let len = list_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "list assignment index out of range",
                        );
                    }
                    let elems = seq_vec(ptr);
                    let old_bits = elems.remove(i as usize);
                    dec_ref_bits(_py, old_bits);
                    return obj_bits;
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let len = bytes_len(ptr) as isize;
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let (start, stop, step) = match normalize_slice_indices(
                            _py, len, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let elems = bytearray_vec(ptr);
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            elems.drain(s..e);
                            return obj_bits;
                        }
                        let indices = collect_slice_indices(start, stop, step);
                        if step > 0 {
                            for &idx in indices.iter().rev() {
                                elems.remove(idx);
                            }
                        } else {
                            for &idx in indices.iter() {
                                elems.remove(idx);
                            }
                        }
                        return obj_bits;
                    }
                    // `__index__`-only key coercion (see `molt_index`): a float
                    // key raises TypeError, it is not truncated.
                    let Some(idx) = sequence_index_i64(_py, key_bits, "bytearray") else {
                        return MoltObject::none().bits();
                    };
                    let len = bytes_len(ptr) as i64;
                    let mut i = idx;
                    if i < 0 {
                        i += len;
                    }
                    if i < 0 || i >= len {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "bytearray index out of range",
                        );
                    }
                    let elems = bytearray_vec(ptr);
                    elems.remove(i as usize);
                    return obj_bits;
                }
                if type_id == TYPE_ID_MEMORYVIEW {
                    if memoryview_released(ptr) {
                        return raise_released_memoryview(_py);
                    }
                    if memoryview_readonly(ptr) {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "cannot modify read-only memory",
                        );
                    }
                    return raise_exception::<_>(_py, "TypeError", "cannot delete memory");
                }
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    let removed = dict_del_in_place(_py, dict_ptr, key_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if removed {
                        return obj_bits;
                    }
                    return raise_key_error_with_key(_py, key_bits);
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__delitem__") {
                    if let Some(call_bits) = attr_lookup_ptr(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        exception_stack_push();
                        let _ = call_callable1(_py, call_bits, key_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            exception_stack_pop(_py);
                            return MoltObject::none().bits();
                        }
                        exception_stack_pop(_py);
                        return obj_bits;
                    }
                    dec_ref_bits(_py, name_bits);
                }
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getitem_method(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_index(obj_bits, key_bits) })
}

/// Same as `molt_getitem_method` but the caller guarantees the index is
/// non-negative and within bounds (proven by the BCE pass).  Currently
/// delegates to `molt_index` which already has type-dispatch fast paths;
/// a future refinement can skip the bounds-check branch entirely for
/// list types once the hot-path is profiled.
#[unsafe(no_mangle)]
pub extern "C" fn molt_getitem_unchecked(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { molt_index(obj_bits, key_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_setitem_method(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    dict_set_in_place(_py, dict_ptr, key_bits, val_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        let _ = molt_store_index(obj_bits, key_bits, val_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_delitem_method(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let _ = molt_del_index(obj_bits, key_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contains(container_bits: u64, item_bits: u64) -> u64 {
    // Tolerate None container from undefined SSA paths on exception handler branches.
    if obj_from_bits(container_bits).is_none() {
        return MoltObject::from_bool(false).bits();
    }
    crate::with_gil_entry_nopanic!(_py, {
        let container = obj_from_bits(container_bits);
        let item = obj_from_bits(item_bits);
        if let Some(ptr) = container.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if !ensure_hashable(_py, item_bits, HashContext::DictKey) {
                        return MoltObject::none().bits();
                    }
                    let order = dict_order(dict_ptr);
                    let hashes = dict_hashes(dict_ptr);
                    let table = dict_table(dict_ptr);
                    let found = dict_find_entry(_py, order, hashes, table, item_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_bool(found.is_some()).bits();
                }
                match type_id {
                    TYPE_ID_LIST => {
                        // Fast path: for NaN-boxed integers/bools/None, identity
                        // (bit-equality) implies value equality. Scan the raw u64
                        // slice first to avoid per-element inc_ref/eq/dec_ref.
                        // This is the hot path for `x in [1, 2, 3]` style range checks.
                        if item.as_int().is_some() || item.is_bool() || item.is_none() {
                            let elems = seq_vec_ref(ptr);
                            if simd_contains_u64(elems, item_bits) {
                                return MoltObject::from_bool(true).bits();
                            }
                            return MoltObject::from_bool(false).bits();
                        }
                        let mut idx = 0usize;
                        while let Some(val) = list_elem_at(ptr, idx) {
                            let elem_bits = val;
                            // Identity check: bit-equality implies identity.
                            // Non-NaN floats are stored inline with unique bit
                            // patterns; NaN floats are heap-allocated with unique
                            // pointer addresses. Both cases make bit-equality
                            // correct for identity.
                            if elem_bits == item_bits {
                                return MoltObject::from_bool(true).bits();
                            }
                            inc_ref_bits(_py, elem_bits);
                            let eq = match eq_bool_from_bits(_py, elem_bits, item_bits) {
                                Some(val) => val,
                                None => {
                                    dec_ref_bits(_py, elem_bits);
                                    return MoltObject::none().bits();
                                }
                            };
                            dec_ref_bits(_py, elem_bits);
                            if eq {
                                return MoltObject::from_bool(true).bits();
                            }
                            idx += 1;
                        }
                        return MoltObject::from_bool(false).bits();
                    }
                    TYPE_ID_LIST_INT => {
                        // list[int] stores raw i64 — compare against the
                        // needle's int value directly for O(n) scan.
                        let elems = crate::object::layout::list_int_vec_ref(ptr);
                        if let Some(needle) = to_i64(item) {
                            for &raw in elems.iter() {
                                if raw == needle {
                                    return MoltObject::from_bool(true).bits();
                                }
                            }
                            return MoltObject::from_bool(false).bits();
                        }
                        // Non-int needle: box each element and compare.
                        for &raw in elems.iter() {
                            let elem_bits = MoltObject::from_int(raw).bits();
                            let eq = match eq_bool_from_bits(_py, elem_bits, item_bits) {
                                Some(val) => val,
                                None => return MoltObject::none().bits(),
                            };
                            if eq {
                                return MoltObject::from_bool(true).bits();
                            }
                        }
                        return MoltObject::from_bool(false).bits();
                    }
                    TYPE_ID_LIST_BOOL => {
                        // list[bool] stores raw u8 (0/1) — compare against
                        // the needle's bool value directly.
                        let elems = crate::object::layout::list_bool_vec_ref(ptr);
                        if let Some(needle_bool) = item.as_bool() {
                            let needle_u8: u8 = if needle_bool { 1 } else { 0 };
                            for &raw in elems.iter() {
                                if raw == needle_u8 {
                                    return MoltObject::from_bool(true).bits();
                                }
                            }
                            return MoltObject::from_bool(false).bits();
                        }
                        // Non-bool needle (e.g. int 1 == True): box each element.
                        for &raw in elems.iter() {
                            let elem_bits = MoltObject::from_bool(raw != 0).bits();
                            let eq = match eq_bool_from_bits(_py, elem_bits, item_bits) {
                                Some(val) => val,
                                None => return MoltObject::none().bits(),
                            };
                            if eq {
                                return MoltObject::from_bool(true).bits();
                            }
                        }
                        return MoltObject::from_bool(false).bits();
                    }
                    TYPE_ID_TUPLE => {
                        let elems = seq_vec_ref(ptr);
                        // Same identity fast path for tuples with inline-int/bool/None needle.
                        if item.as_int().is_some() || item.is_bool() || item.is_none() {
                            if simd_contains_u64(elems, item_bits) {
                                return MoltObject::from_bool(true).bits();
                            }
                            return MoltObject::from_bool(false).bits();
                        }
                        for &elem_bits in elems.iter() {
                            if elem_bits == item_bits {
                                return MoltObject::from_bool(true).bits();
                            }
                            let eq = match eq_bool_from_bits(_py, elem_bits, item_bits) {
                                Some(val) => val,
                                None => return MoltObject::none().bits(),
                            };
                            if eq {
                                return MoltObject::from_bool(true).bits();
                            }
                        }
                        return MoltObject::from_bool(false).bits();
                    }
                    TYPE_ID_SET | TYPE_ID_FROZENSET => {
                        if !ensure_hashable(_py, item_bits, HashContext::SetElement) {
                            return MoltObject::none().bits();
                        }
                        let order = set_order(ptr);
                        let hashes = set_hashes(ptr);
                        let table = set_table(ptr);
                        let found = set_find_entry(_py, order, hashes, table, item_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        return MoltObject::from_bool(found.is_some()).bits();
                    }
                    TYPE_ID_STRING => {
                        let Some(item_ptr) = item.as_ptr() else {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                &format!(
                                    "'in <string>' requires string as left operand, not {}",
                                    type_name(_py, item)
                                ),
                            );
                        };
                        if object_type_id(item_ptr) != TYPE_ID_STRING {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                &format!(
                                    "'in <string>' requires string as left operand, not {}",
                                    type_name(_py, item)
                                ),
                            );
                        }
                        let hay_len = string_len(ptr);
                        let needle_len = string_len(item_ptr);
                        let hay_bytes = std::slice::from_raw_parts(string_bytes(ptr), hay_len);
                        let needle_bytes =
                            std::slice::from_raw_parts(string_bytes(item_ptr), needle_len);
                        if needle_bytes.is_empty() {
                            return MoltObject::from_bool(true).bits();
                        }
                        let idx = bytes_find_impl(hay_bytes, needle_bytes);
                        return MoltObject::from_bool(idx >= 0).bits();
                    }
                    TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
                        let hay_len = bytes_len(ptr);
                        let hay_bytes = std::slice::from_raw_parts(bytes_data(ptr), hay_len);
                        if let Some(byte) = item.as_int() {
                            if !(0..=255).contains(&byte) {
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "byte must be in range(0, 256)",
                                );
                            }
                            let found = memchr(byte as u8, hay_bytes).is_some();
                            return MoltObject::from_bool(found).bits();
                        }
                        if let Some(item_ptr) = item.as_ptr() {
                            let item_type = object_type_id(item_ptr);
                            if item_type == TYPE_ID_BYTES || item_type == TYPE_ID_BYTEARRAY {
                                let needle_len = bytes_len(item_ptr);
                                let needle_bytes =
                                    std::slice::from_raw_parts(bytes_data(item_ptr), needle_len);
                                if needle_bytes.is_empty() {
                                    return MoltObject::from_bool(true).bits();
                                }
                                let idx = bytes_find_impl(hay_bytes, needle_bytes);
                                return MoltObject::from_bool(idx >= 0).bits();
                            }
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            &format!(
                                "a bytes-like object is required, not '{}'",
                                type_name(_py, item)
                            ),
                        );
                    }
                    TYPE_ID_RANGE => {
                        let candidate = if let Some(f) = as_float_extended(item) {
                            if !f.is_finite() || f.fract() != 0.0 {
                                return MoltObject::from_bool(false).bits();
                            }
                            bigint_from_f64_trunc(f)
                        } else {
                            let type_err = format!(
                                "'{}' object cannot be interpreted as an integer",
                                type_name(_py, item)
                            );
                            let Some(val) = index_bigint_from_obj(_py, item_bits, &type_err) else {
                                if exception_pending(_py) {
                                    molt_exception_clear();
                                }
                                return MoltObject::from_bool(false).bits();
                            };
                            val
                        };
                        let Some((start, stop, step)) = range_components_bigint(ptr) else {
                            return MoltObject::none().bits();
                        };
                        if step.is_zero() {
                            return MoltObject::from_bool(false).bits();
                        }
                        let in_range = if step.is_positive() {
                            candidate >= start && candidate < stop
                        } else {
                            candidate <= start && candidate > stop
                        };
                        if !in_range {
                            return MoltObject::from_bool(false).bits();
                        }
                        let offset = candidate - start;
                        let step_abs = if step.is_negative() { -step } else { step };
                        let aligned = offset.mod_floor(&step_abs).is_zero();
                        return MoltObject::from_bool(aligned).bits();
                    }
                    TYPE_ID_MEMORYVIEW => {
                        if memoryview_released(ptr) {
                            return raise_released_memoryview(_py);
                        }
                        let data = memoryview_data(ptr);
                        if data.is_null() {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                &format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, item)
                                ),
                            );
                        }
                        let len = memoryview_len(ptr);
                        let itemsize = memoryview_itemsize(ptr);
                        let stride = memoryview_stride(ptr);
                        if itemsize != 1 {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "memoryview itemsize not supported",
                            );
                        }
                        if stride == 1 {
                            let hay = std::slice::from_raw_parts(data.cast_const(), len);
                            if let Some(byte) = item.as_int() {
                                if !(0..=255).contains(&byte) {
                                    return raise_exception::<_>(
                                        _py,
                                        "ValueError",
                                        "byte must be in range(0, 256)",
                                    );
                                }
                                let found = memchr(byte as u8, hay).is_some();
                                return MoltObject::from_bool(found).bits();
                            }
                            if let Some(item_ptr) = item.as_ptr() {
                                let item_type = object_type_id(item_ptr);
                                if item_type == TYPE_ID_BYTES || item_type == TYPE_ID_BYTEARRAY {
                                    let needle_len = bytes_len(item_ptr);
                                    let needle_bytes = std::slice::from_raw_parts(
                                        bytes_data(item_ptr),
                                        needle_len,
                                    );
                                    if needle_bytes.is_empty() {
                                        return MoltObject::from_bool(true).bits();
                                    }
                                    let idx = bytes_find_impl(hay, needle_bytes);
                                    return MoltObject::from_bool(idx >= 0).bits();
                                }
                            }
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                &format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, item)
                                ),
                            );
                        }
                        let mut out = Vec::with_capacity(len);
                        for idx in 0..len {
                            let start = (idx as isize) * stride;
                            if start < 0 {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    &format!(
                                        "a bytes-like object is required, not '{}'",
                                        type_name(_py, item)
                                    ),
                                );
                            }
                            out.push(*data.offset(start));
                        }
                        let hay = out.as_slice();
                        if let Some(byte) = item.as_int() {
                            if !(0..=255).contains(&byte) {
                                return raise_exception::<_>(
                                    _py,
                                    "ValueError",
                                    "byte must be in range(0, 256)",
                                );
                            }
                            let found = memchr(byte as u8, hay).is_some();
                            return MoltObject::from_bool(found).bits();
                        }
                        if let Some(item_ptr) = item.as_ptr() {
                            let item_type = object_type_id(item_ptr);
                            if item_type == TYPE_ID_BYTES || item_type == TYPE_ID_BYTEARRAY {
                                let needle_len = bytes_len(item_ptr);
                                let needle_bytes =
                                    std::slice::from_raw_parts(bytes_data(item_ptr), needle_len);
                                if needle_bytes.is_empty() {
                                    return MoltObject::from_bool(true).bits();
                                }
                                let idx = bytes_find_impl(hay, needle_bytes);
                                return MoltObject::from_bool(idx >= 0).bits();
                            }
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            &format!(
                                "a bytes-like object is required, not '{}'",
                                type_name(_py, item)
                            ),
                        );
                    }
                    _ => {}
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__contains__") {
                    if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        let res_bits = call_callable1(_py, call_bits, item_bits);
                        dec_ref_bits(_py, call_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        if !is_not_implemented_bits(_py, res_bits) {
                            let truthy = is_truthy(_py, obj_from_bits(res_bits));
                            dec_ref_bits(_py, res_bits);
                            return MoltObject::from_bool(truthy).bits();
                        }
                        dec_ref_bits(_py, res_bits);
                    } else {
                        dec_ref_bits(_py, name_bits);
                    }
                }
                let iter_bits = molt_iter(container_bits);
                if !obj_from_bits(iter_bits).is_none() {
                    loop {
                        let pair_bits = molt_iter_next(iter_bits);
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                        let pair_obj = obj_from_bits(pair_bits);
                        let Some(pair_ptr) = pair_obj.as_ptr() else {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "object is not an iterator",
                            );
                        };
                        if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "object is not an iterator",
                            );
                        }
                        let elems = seq_vec_ref(pair_ptr);
                        if elems.len() < 2 {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "object is not an iterator",
                            );
                        }
                        let val_bits = elems[0];
                        let done_bits = elems[1];
                        if is_truthy(_py, obj_from_bits(done_bits)) {
                            return MoltObject::from_bool(false).bits();
                        }
                        if obj_eq(_py, obj_from_bits(val_bits), item) {
                            return MoltObject::from_bool(true).bits();
                        }
                    }
                }
                if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__getitem__") {
                    if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits) {
                        dec_ref_bits(_py, name_bits);
                        let mut idx = 0i64;
                        loop {
                            let idx_bits = MoltObject::from_int(idx).bits();
                            exception_stack_push();
                            let val_bits = call_callable1(_py, call_bits, idx_bits);
                            if exception_pending(_py) {
                                let exc_bits = molt_exception_last();
                                let exc_obj = obj_from_bits(exc_bits);
                                let mut is_index_error = false;
                                if let Some(exc_ptr) = exc_obj.as_ptr()
                                    && object_type_id(exc_ptr) == TYPE_ID_EXCEPTION
                                {
                                    let kind_bits = exception_kind_bits(exc_ptr);
                                    let kind_obj = obj_from_bits(kind_bits);
                                    if let Some(kind_ptr) = kind_obj.as_ptr()
                                        && object_type_id(kind_ptr) == TYPE_ID_STRING
                                    {
                                        let bytes = std::slice::from_raw_parts(
                                            string_bytes(kind_ptr),
                                            string_len(kind_ptr),
                                        );
                                        if bytes == b"IndexError" {
                                            is_index_error = true;
                                        }
                                    }
                                }
                                if is_index_error {
                                    clear_exception(_py);
                                    exception_stack_pop(_py);
                                    dec_ref_bits(_py, exc_bits);
                                    return MoltObject::from_bool(false).bits();
                                }
                                exception_stack_pop_restore_last(_py, exc_bits);
                                dec_ref_bits(_py, exc_bits);
                                return MoltObject::none().bits();
                            }
                            exception_stack_pop(_py);
                            if obj_eq(_py, obj_from_bits(val_bits), item) {
                                dec_ref_bits(_py, val_bits);
                                return MoltObject::from_bool(true).bits();
                            }
                            dec_ref_bits(_py, val_bits);
                            idx += 1;
                        }
                    } else {
                        dec_ref_bits(_py, name_bits);
                    }
                }
            }
        }
        raise_exception::<_>(
            _py,
            "TypeError",
            &format!(
                "argument of type '{}' is not iterable",
                type_name(_py, container)
            ),
        )
    })
}

/// Specialized `in` for list containers (linear scan, no type dispatch).
#[unsafe(no_mangle)]
pub extern "C" fn molt_list_contains(container_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let container = obj_from_bits(container_bits);
        if let Some(ptr) = container.as_ptr() {
            unsafe {
                let mut idx = 0usize;
                while let Some(val) = list_elem_at(ptr, idx) {
                    let elem_bits = val;
                    if elem_bits == item_bits {
                        return MoltObject::from_bool(true).bits();
                    }
                    inc_ref_bits(_py, elem_bits);
                    let eq = match eq_bool_from_bits(_py, elem_bits, item_bits) {
                        Some(val) => val,
                        None => {
                            dec_ref_bits(_py, elem_bits);
                            return MoltObject::none().bits();
                        }
                    };
                    dec_ref_bits(_py, elem_bits);
                    if eq {
                        return MoltObject::from_bool(true).bits();
                    }
                    idx += 1;
                }
                return MoltObject::from_bool(false).bits();
            }
        }
        molt_contains(container_bits, item_bits)
    })
}

/// Specialized `in` for str containers (substring search, no type dispatch).
#[unsafe(no_mangle)]
pub extern "C" fn molt_str_contains(container_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let container = obj_from_bits(container_bits);
        let item = obj_from_bits(item_bits);
        if let Some(ptr) = container.as_ptr() {
            unsafe {
                let Some(item_ptr) = item.as_ptr() else {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!(
                            "'in <string>' requires string as left operand, not {}",
                            type_name(_py, item)
                        ),
                    );
                };
                if object_type_id(item_ptr) != TYPE_ID_STRING {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!(
                            "'in <string>' requires string as left operand, not {}",
                            type_name(_py, item)
                        ),
                    );
                }
                let hay_len = string_len(ptr);
                let needle_len = string_len(item_ptr);
                let hay_bytes = std::slice::from_raw_parts(string_bytes(ptr), hay_len);
                let needle_bytes = std::slice::from_raw_parts(string_bytes(item_ptr), needle_len);
                if needle_bytes.is_empty() {
                    return MoltObject::from_bool(true).bits();
                }
                let idx = bytes_find_impl(hay_bytes, needle_bytes);
                return MoltObject::from_bool(idx >= 0).bits();
            }
        }
        molt_contains(container_bits, item_bits)
    })
}
