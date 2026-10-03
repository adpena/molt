use super::*;
use molt_cpython_abi::api::errors::with_preserved_error;

#[cfg(test)]
mod comparison_consumer_tests;

pub(crate) fn value_supports_mp_subscript(_py: &PyToken<'_>, obj_bits: u64) -> bool {
    if let Some(ptr) = obj_from_bits(obj_bits).as_ptr()
        && unsafe { object_type_id(ptr) } == crate::TYPE_ID_FOREIGN
    {
        let native = unsafe { crate::object::foreign::foreign_ptr_from_obj(ptr) };
        let status = unsafe {
            molt_cpython_abi::api::abstract_mapping::PyMapping_Check(
                std::ptr::with_exposed_provenance_mut(native),
            )
        };
        if !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null()
            || exception_pending(_py)
        {
            crate::cpython_abi_hooks::propagate_native_failure(_py, "native mapping admission");
            return false;
        }
        return status != 0;
    }
    unsafe { crate::builtins::attr::has_special_method(_py, obj_bits, b"__getitem__") }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_index(obj_bits: u64, key_bits: u64) -> u64 {
    index_impl(obj_bits, key_bits, false)
}

pub(crate) extern "C" fn molt_getitem_builtin(obj_bits: u64, key_bits: u64) -> u64 {
    index_impl(obj_bits, key_bits, true)
}

fn index_impl(obj_bits: u64, key_bits: u64, builtin_only: bool) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        // Fast path: dict[key] — skips exception_pending and type dispatch chain.
        if let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() {
            unsafe {
                if !builtin_only
                    && (object_type_id(obj_ptr) == crate::TYPE_ID_FOREIGN
                        || !crate::object::iterable::builtin_receiver(_py, obj_ptr))
                {
                    if let Some(method) =
                        crate::builtins::attr::lookup_special_method(_py, obj_bits, b"__getitem__")
                    {
                        let result = call_callable1(_py, method, key_bits);
                        with_preserved_error(|| dec_ref_bits(_py, method));
                        return result;
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    // Classes also support __class_getitem__; ordinary values
                    // have exhausted their live protocol at this boundary.
                    if object_type_id(obj_ptr) != TYPE_ID_TYPE {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            &format!(
                                "'{}' object is not subscriptable",
                                type_name(_py, obj_from_bits(obj_bits))
                            ),
                        );
                    }
                }
                if object_is_exact_builtin_dict(_py, obj_ptr) {
                    if let Some(val) = dict_get_in_place(_py, obj_ptr, key_bits) {
                        if obj_from_bits(val).as_ptr().is_some() {
                            inc_ref_bits(_py, val);
                        }
                        return val;
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
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
                        let len = crate::object::seq_access::len(obj_ptr) as i64;
                        let raw = key.as_int_unchecked();
                        let idx = if raw < 0 { raw + len } else { raw };
                        if idx >= 0 && idx < len {
                            let Some(val) = crate::object::seq_access::item(obj_ptr, idx as usize)
                            else {
                                return MoltObject::none().bits();
                            };
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
                        None => {
                            let format =
                                string_obj_to_owned(obj_from_bits(memoryview_format_bits(ptr)))
                                    .unwrap_or_default();
                            return raise_exception::<_>(
                                _py,
                                "NotImplementedError",
                                &format!("memoryview: unsupported format {format}"),
                            );
                        }
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
                            && crate::object::seq_access::with_immutable_tuple_slice(
                                tup_ptr,
                                |elems| elems.is_empty(),
                            )
                            .unwrap_or(false)
                        {
                            let val = memoryview_read_scalar_at(_py, ptr, 0, fmt);
                            return val.unwrap_or_else(|| MoltObject::none().bits());
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
                        let Some(elems) = crate::object::seq_access::snapshot(
                            _py,
                            tup_ptr,
                            "memoryview index tuple snapshot allocation failed",
                        ) else {
                            return MoltObject::none().bits();
                        };
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
                        let mut indices = Vec::with_capacity(elems.len());
                        for (dim, &elem_bits) in elems.iter().enumerate() {
                            let Some(idx) = sequence_index_i64_with_type_error(
                                _py,
                                elem_bits,
                                "memoryview: invalid slice key",
                            ) else {
                                return MoltObject::none().bits();
                            };
                            if memoryview_released(ptr) {
                                return raise_released_memoryview(_py);
                            }
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
                            indices.push(i as isize);
                        }
                        let Some(pos) = memoryview_strided_offset(&indices, strides) else {
                            return MoltObject::none().bits();
                        };
                        let val = memoryview_read_scalar_at(_py, ptr, pos, fmt);
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
                        if memoryview_released(ptr) {
                            return raise_released_memoryview(_py);
                        }
                        let base_stride = strides[0];
                        let itemsize = memoryview_itemsize(ptr);
                        let new_len = range_len_i64(start as i64, stop as i64, step as i64);
                        let new_len = new_len.max(0) as usize;
                        let start_delta = if new_len == 0 {
                            0
                        } else {
                            if start < 0 {
                                return MoltObject::none().bits();
                            }
                            let Some(start_delta) =
                                memoryview_linear_offset(start as usize, base_stride)
                            else {
                                return MoltObject::none().bits();
                            };
                            start_delta
                        };
                        let Some(new_offset) = base_offset.checked_add(start_delta) else {
                            return MoltObject::none().bits();
                        };
                        let Some(new_stride) = base_stride.checked_mul(step) else {
                            return MoltObject::none().bits();
                        };
                        let mut new_shape = shape.to_vec();
                        let mut new_strides = strides.to_vec();
                        if !new_shape.is_empty() {
                            new_shape[0] = new_len as isize;
                            new_strides[0] = new_stride;
                        }
                        let storage = TypedStridedStorage::new(
                            data.offset(start_delta),
                            memoryview_readonly(ptr),
                            itemsize,
                            new_offset,
                            memoryview_base_bits(ptr),
                            memoryview_format_bits(ptr),
                            new_shape,
                            new_strides,
                        )
                        .map(|storage| {
                            storage
                                .with_owner(memoryview_owner_bits(ptr))
                                .with_native_lease((*memoryview_ptr(ptr)).native_lease.clone())
                        });
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
                    if memoryview_released(ptr) {
                        return raise_released_memoryview(_py);
                    }
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
                    let Some(pos) = memoryview_linear_offset(i as usize, strides[0]) else {
                        return MoltObject::none().bits();
                    };
                    let val = memoryview_read_scalar_at(_py, ptr, pos, fmt);
                    return val.unwrap_or_else(|| MoltObject::none().bits());
                }
                if type_id == TYPE_ID_STRING
                    || type_id == TYPE_ID_BYTES
                    || type_id == TYPE_ID_BYTEARRAY
                {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let slice = match crate::object::ops_sys::DecodedSlice::decode(
                            _py, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(slice) => slice,
                            Err(err) => return slice_error(_py, err),
                        };
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
                        let (start, stop, step) = slice.adjust(len);
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
                        let slice = match crate::object::ops_sys::DecodedSlice::decode(
                            _py,
                            obj_from_bits(slice_start_bits(slice_ptr)),
                            obj_from_bits(slice_stop_bits(slice_ptr)),
                            obj_from_bits(slice_step_bits(slice_ptr)),
                        ) {
                            Ok(slice) => slice,
                            Err(err) => return slice_error(_py, err),
                        };
                        let Some(elems) = crate::object::seq_access::snapshot(
                            _py,
                            ptr,
                            "list slice snapshot allocation failed",
                        ) else {
                            return MoltObject::none().bits();
                        };
                        let (start, stop, step) = slice.adjust(elems.len() as isize);
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
                    let Some(val_item) = crate::object::seq_access::pin_item(_py, ptr, i as usize)
                    else {
                        return raise_exception::<_>(_py, "IndexError", "list index out of range");
                    };
                    let val = val_item.bits();
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
                    drop(val_item);
                    return val;
                }
                if type_id == TYPE_ID_TUPLE {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let Some(elems) = crate::object::seq_access::snapshot(
                            _py,
                            ptr,
                            "tuple slice snapshot allocation failed",
                        ) else {
                            return MoltObject::none().bits();
                        };
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
                    let Some(val) = crate::object::seq_access::item(ptr, i as usize) else {
                        return raise_exception::<_>(_py, "IndexError", "tuple index out of range");
                    };
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
                            return int_bits_from_i64(_py, value);
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
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    if !object_is_exact_builtin_dict(_py, ptr) {
                        if let Some(call_bits) = crate::builtins::attr::lookup_special_method(
                            _py,
                            obj_bits,
                            b"__missing__",
                        ) {
                            exception_stack_push();
                            let res = call_callable1(_py, call_bits, key_bits);
                            with_preserved_error(|| dec_ref_bits(_py, call_bits));
                            if exception_pending(_py) {
                                with_preserved_error(|| dec_ref_bits(_py, res));
                                exception_stack_pop(_py);
                                return MoltObject::none().bits();
                            }
                            exception_stack_pop(_py);
                            return res;
                        }
                        if exception_pending(_py) {
                            return MoltObject::none().bits();
                        }
                    }
                    return raise_key_error_with_key(_py, key_bits);
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
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
                            with_preserved_error(|| dec_ref_bits(_py, call_bits));
                            if exception_pending(_py) {
                                with_preserved_error(|| dec_ref_bits(_py, res));
                                exception_stack_pop(_py);
                                return MoltObject::none().bits();
                            }
                            exception_stack_pop(_py);
                            return res;
                        }
                        dec_ref_bits(_py, name_bits);
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
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

/// Statement operation: successful builtin mutations return the borrowed
/// container without retaining it. Callers must inspect exception state, not
/// treat the ABI return as a newly owned value (StoreIndex has zero IR results).
#[unsafe(no_mangle)]
pub extern "C" fn molt_store_index(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    store_index_impl(obj_bits, key_bits, val_bits, false)
}

pub(crate) extern "C" fn molt_setitem_builtin(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    store_index_impl(obj_bits, key_bits, val_bits, true)
}

pub(crate) extern "C" fn molt_setitem_builtin_method(
    obj_bits: u64,
    key_bits: u64,
    val_bits: u64,
) -> u64 {
    let _ = molt_setitem_builtin(obj_bits, key_bits, val_bits);
    MoltObject::none().bits()
}

fn store_index_impl(obj_bits: u64, key_bits: u64, val_bits: u64, builtin_only: bool) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        // Fast path: dict[key] = val — skips type dispatch chain.
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if !builtin_only
                    && (object_type_id(ptr) == crate::TYPE_ID_FOREIGN
                        || !crate::object::iterable::builtin_receiver(_py, ptr))
                {
                    if let Some(method) =
                        crate::builtins::attr::lookup_special_method(_py, obj_bits, b"__setitem__")
                    {
                        let result = call_callable2(_py, method, key_bits, val_bits);
                        with_preserved_error(|| {
                            dec_ref_bits(_py, method);
                            dec_ref_bits(_py, result);
                        });
                        return if exception_pending(_py) {
                            MoltObject::none().bits()
                        } else {
                            obj_bits
                        };
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return unsupported_item_mutation(_py, obj_bits, false);
                }
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
                        let slice = match crate::object::ops_sys::DecodedSlice::decode(
                            _py,
                            obj_from_bits(slice_start_bits(slice_ptr)),
                            obj_from_bits(slice_stop_bits(slice_ptr)),
                            obj_from_bits(slice_step_bits(slice_ptr)),
                        ) {
                            Ok(slice) => slice,
                            Err(err) => return slice_error(_py, err),
                        };
                        let (start, stop, step) = slice.adjust(list_len(ptr) as isize);
                        let new_items = match collect_iterable_values(
                            _py,
                            val_bits,
                            "must assign iterable to extended slice",
                        ) {
                            Some(items) => items,
                            None => return MoltObject::none().bits(),
                        };
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            let changed = crate::object::list_mutation::replace_range(
                                _py, ptr, s, e, &new_items,
                            );
                            for item in new_items {
                                dec_ref_bits(_py, item);
                            }
                            if !changed {
                                return MoltObject::none().bits();
                            }
                            return obj_bits;
                        }
                        let indices = collect_slice_indices(start, stop, step);
                        if indices.len() != new_items.len() {
                            let new_len = new_items.len();
                            for item in new_items {
                                dec_ref_bits(_py, item);
                            }
                            return raise_exception::<_>(
                                _py,
                                "ValueError",
                                &format!(
                                    "attempt to assign sequence of size {} to extended slice of size {}",
                                    new_len,
                                    indices.len()
                                ),
                            );
                        }
                        let changed = crate::object::list_mutation::replace_indices(
                            _py, ptr, &indices, &new_items,
                        );
                        for item in new_items {
                            dec_ref_bits(_py, item);
                        }
                        if !changed {
                            return MoltObject::none().bits();
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
                    let index = i as usize;
                    if !crate::object::list_mutation::replace_indices(
                        _py,
                        ptr,
                        std::slice::from_ref(&index),
                        std::slice::from_ref(&val_bits),
                    ) {
                        return MoltObject::none().bits();
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
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let slice = match crate::object::ops_sys::DecodedSlice::decode(
                            _py, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let src_bytes = match collect_bytearray_assign_bytes(_py, val_bits) {
                            Some(bytes) => bytes,
                            None => return MoltObject::none().bits(),
                        };
                        // CPython recurses after materializing non-bytearray
                        // inputs (and self-assignment); slice coercions run a
                        // second time in that case, after RHS callbacks.
                        let source_is_distinct_bytearray =
                            obj_from_bits(val_bits).as_ptr().is_some_and(|source| {
                                source != ptr && object_type_id(source) == TYPE_ID_BYTEARRAY
                            });
                        let slice = if source_is_distinct_bytearray {
                            slice
                        } else {
                            match crate::object::ops_sys::DecodedSlice::decode(
                                _py, start_obj, stop_obj, step_obj,
                            ) {
                                Ok(slice) => slice,
                                Err(err) => return slice_error(_py, err),
                            }
                        };
                        let len = bytearray_len(ptr);
                        let (start, stop, step) = slice.adjust(len as isize);
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            let Some(new_len) = (len - (e - s)).checked_add(src_bytes.len()) else {
                                return raise_exception::<_>(
                                    _py,
                                    "MemoryError",
                                    "bytearray allocation failed",
                                );
                            };
                            if crate::object::buffer_exports::bytearray_mutate(
                                _py,
                                ptr,
                                new_len,
                                |elems| {
                                    elems.splice(s..e, src_bytes.iter().copied());
                                },
                            )
                            .is_none()
                            {
                                return MoltObject::none().bits();
                            }
                            return obj_bits;
                        }
                        let indices = collect_slice_indices(start, stop, step);
                        if src_bytes.is_empty() {
                            if !crate::object::buffer_exports::bytearray_remove_indices(
                                _py, ptr, &indices,
                            ) {
                                return MoltObject::none().bits();
                            }
                            return obj_bits;
                        }
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
                        crate::object::buffer_exports::bytearray_mutate(_py, ptr, len, |elems| {
                            for (idx, byte) in indices.iter().zip(src_bytes.iter()) {
                                elems[*idx] = *byte;
                            }
                        });
                        return obj_bits;
                    }
                    // `__index__`-only key coercion (see `molt_index`): a float
                    // key raises TypeError, it is not truncated.
                    let Some(idx) = sequence_index_i64(_py, key_bits, "bytearray") else {
                        return MoltObject::none().bits();
                    };
                    let Some(byte) = bytes_item_to_u8(_py, val_bits, BytesCtorKind::Bytearray)
                    else {
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
                            && crate::object::seq_access::with_immutable_tuple_slice(
                                tup_ptr,
                                |elems| elems.is_empty(),
                            )
                            .unwrap_or(false)
                        {
                            let ok = memoryview_write_scalar_at(_py, ptr, 0, fmt, val_bits);
                            if ok.is_none() {
                                return MoltObject::none().bits();
                            }
                            return obj_bits;
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
                        let Some(elems) = crate::object::seq_access::snapshot(
                            _py,
                            tup_ptr,
                            "memoryview assignment index tuple snapshot allocation failed",
                        ) else {
                            return MoltObject::none().bits();
                        };
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
                        let mut indices = Vec::with_capacity(elems.len());
                        for (dim, &elem_bits) in elems.iter().enumerate() {
                            let Some(idx) = sequence_index_i64_with_type_error(
                                _py,
                                elem_bits,
                                "memoryview: invalid slice key",
                            ) else {
                                return MoltObject::none().bits();
                            };
                            if memoryview_released(ptr) {
                                return raise_released_memoryview(_py);
                            }
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
                            indices.push(i as isize);
                        }
                        let Some(pos) = memoryview_strided_offset(&indices, strides) else {
                            return MoltObject::none().bits();
                        };
                        let ok = memoryview_write_scalar_at(_py, ptr, pos, fmt, val_bits);
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
                        if memoryview_released(ptr) {
                            return raise_released_memoryview(_py);
                        }
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
                        let base_stride = strides[0];
                        if start < 0 {
                            return MoltObject::none().bits();
                        }
                        let Some(mut pos) = memoryview_linear_offset(start as usize, base_stride)
                        else {
                            return MoltObject::none().bits();
                        };
                        let Some(step_stride) = base_stride.checked_mul(step) else {
                            return MoltObject::none().bits();
                        };
                        let mut idx = 0usize;
                        while idx < src_bytes.len() {
                            let dst =
                                std::slice::from_raw_parts_mut(data.offset(pos), fmt.itemsize);
                            dst.copy_from_slice(&src_bytes[idx..idx + fmt.itemsize]);
                            idx += fmt.itemsize;
                            let Some(next_pos) = pos.checked_add(step_stride) else {
                                return MoltObject::none().bits();
                            };
                            pos = next_pos;
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
                    if memoryview_released(ptr) {
                        return raise_released_memoryview(_py);
                    }
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
                    let Some(pos) = memoryview_linear_offset(i as usize, strides[0]) else {
                        return MoltObject::none().bits();
                    };
                    let ok = memoryview_write_scalar_at(_py, ptr, pos, fmt, val_bits);
                    if ok.is_none() {
                        return MoltObject::none().bits();
                    }
                    return obj_bits;
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
            }
        }
        unsupported_item_mutation(_py, obj_bits, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_del_index(obj_bits: u64, key_bits: u64) -> u64 {
    del_index_impl(obj_bits, key_bits, false)
}

pub(crate) extern "C" fn molt_delitem_builtin(obj_bits: u64, key_bits: u64) -> u64 {
    del_index_impl(obj_bits, key_bits, true)
}

pub(crate) extern "C" fn molt_delitem_builtin_method(obj_bits: u64, key_bits: u64) -> u64 {
    let _ = molt_delitem_builtin(obj_bits, key_bits);
    MoltObject::none().bits()
}

fn del_index_impl(obj_bits: u64, key_bits: u64, builtin_only: bool) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(obj_bits);
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
                if !builtin_only
                    && (object_type_id(ptr) == crate::TYPE_ID_FOREIGN
                        || !crate::object::iterable::builtin_receiver(_py, ptr))
                {
                    if let Some(method) =
                        crate::builtins::attr::lookup_special_method(_py, obj_bits, b"__delitem__")
                    {
                        let result = call_callable1(_py, method, key_bits);
                        with_preserved_error(|| {
                            dec_ref_bits(_py, method);
                            dec_ref_bits(_py, result);
                        });
                        return if exception_pending(_py) {
                            MoltObject::none().bits()
                        } else {
                            obj_bits
                        };
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return unsupported_item_mutation(_py, obj_bits, true);
                }
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
                        let slice = match crate::object::ops_sys::DecodedSlice::decode(
                            _py,
                            obj_from_bits(slice_start_bits(slice_ptr)),
                            obj_from_bits(slice_stop_bits(slice_ptr)),
                            obj_from_bits(slice_step_bits(slice_ptr)),
                        ) {
                            Ok(slice) => slice,
                            Err(err) => return slice_error(_py, err),
                        };
                        let (start, stop, step) = slice.adjust(list_len(ptr) as isize);
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            if !crate::object::list_mutation::replace_range(_py, ptr, s, e, &[]) {
                                return MoltObject::none().bits();
                            }
                            return obj_bits;
                        }
                        let mut indices = collect_slice_indices(start, stop, step);
                        if step < 0 {
                            indices.reverse();
                        }
                        if !crate::object::list_mutation::remove_indices(_py, ptr, &indices) {
                            return MoltObject::none().bits();
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
                    let index = i as usize;
                    if !crate::object::list_mutation::replace_range(_py, ptr, index, index + 1, &[])
                    {
                        return MoltObject::none().bits();
                    }
                    return obj_bits;
                }
                if type_id == TYPE_ID_BYTEARRAY {
                    if let Some(slice_ptr) = key.as_ptr()
                        && object_type_id(slice_ptr) == TYPE_ID_SLICE
                    {
                        let start_obj = obj_from_bits(slice_start_bits(slice_ptr));
                        let stop_obj = obj_from_bits(slice_stop_bits(slice_ptr));
                        let step_obj = obj_from_bits(slice_step_bits(slice_ptr));
                        let slice = match crate::object::ops_sys::DecodedSlice::decode(
                            _py, start_obj, stop_obj, step_obj,
                        ) {
                            Ok(vals) => vals,
                            Err(err) => return slice_error(_py, err),
                        };
                        let len = bytearray_len(ptr);
                        let (start, stop, step) = slice.adjust(len as isize);
                        if step == 1 {
                            let s = start as usize;
                            let mut e = stop as usize;
                            if s > e {
                                e = s;
                            }
                            if crate::object::buffer_exports::bytearray_mutate(
                                _py,
                                ptr,
                                len - (e - s),
                                |elems| {
                                    elems.drain(s..e);
                                },
                            )
                            .is_none()
                            {
                                return MoltObject::none().bits();
                            }
                            return obj_bits;
                        }
                        let indices = collect_slice_indices(start, stop, step);
                        if !crate::object::buffer_exports::bytearray_remove_indices(
                            _py, ptr, &indices,
                        ) {
                            return MoltObject::none().bits();
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
                    if crate::object::buffer_exports::bytearray_mutate(
                        _py,
                        ptr,
                        len as usize - 1,
                        |elems| elems.remove(i as usize),
                    )
                    .is_none()
                    {
                        return MoltObject::none().bits();
                    }
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
            }
        }
        unsupported_item_mutation(_py, obj_bits, true)
    })
}

fn unsupported_item_mutation(py: &PyToken<'_>, object: u64, delete: bool) -> u64 {
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let operation = if delete { "deletion" } else { "assignment" };
    raise_exception::<_>(
        py,
        "TypeError",
        &format!(
            "'{}' object does not support item {operation}",
            type_name(py, obj_from_bits(object))
        ),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_getitem_method(obj_bits: u64, key_bits: u64) -> u64 {
    molt_index(obj_bits, key_bits)
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
    contains_impl(container_bits, item_bits, false)
}

pub(crate) extern "C" fn molt_contains_builtin(container_bits: u64, item_bits: u64) -> u64 {
    contains_impl(container_bits, item_bits, true)
}

/// Builtin sequence membership reads one owned item at a time. A callback can
/// resize or promote a compact list, so reload its representation on each step.
unsafe fn sequence_contains_builtin(py: &PyToken<'_>, ptr: *mut u8, needle: u64) -> u64 {
    let item = obj_from_bits(needle);
    let inline_integer = item.as_int().or_else(|| item.as_bool().map(i64::from));
    if let Some(needle) = inline_integer {
        match unsafe { object_type_id(ptr) } {
            TYPE_ID_LIST_INT => {
                let found = unsafe { crate::object::layout::list_int_vec_ref(ptr) }
                    .iter()
                    .any(|&value| value == needle);
                return MoltObject::from_bool(found).bits();
            }
            TYPE_ID_LIST_BOOL => {
                let found = unsafe { crate::object::layout::list_bool_vec_ref(ptr) }
                    .iter()
                    .any(|&value| i64::from(value != 0) == needle);
                return MoltObject::from_bool(found).bits();
            }
            _ => {}
        }
    }
    let mut index = 0;
    loop {
        let value = match unsafe { object_type_id(ptr) } {
            TYPE_ID_LIST | TYPE_ID_TUPLE => {
                let Some(item) = (unsafe { crate::object::seq_access::pin_item(py, ptr, index) })
                else {
                    return MoltObject::from_bool(false).bits();
                };
                item.into_bits()
            }
            TYPE_ID_LIST_INT => {
                let raw = unsafe { crate::object::layout::list_int_vec_ref(ptr) }
                    .as_slice()
                    .get(index)
                    .copied();
                let Some(raw) = raw else {
                    return MoltObject::from_bool(false).bits();
                };
                int_bits_from_i64(py, raw)
            }
            TYPE_ID_LIST_BOOL => {
                let raw = unsafe { crate::object::layout::list_bool_vec_ref(ptr) }
                    .as_slice()
                    .get(index)
                    .copied();
                let Some(raw) = raw else {
                    return MoltObject::from_bool(false).bits();
                };
                MoltObject::from_bool(raw != 0).bits()
            }
            _ => {
                return raise_exception::<_>(
                    py,
                    "SystemError",
                    "invalid sequence membership storage",
                );
            }
        };
        if exception_pending(py) {
            dec_ref_bits(py, value);
            return MoltObject::none().bits();
        }
        let outcome =
            crate::object::ops_compare::compare_object_eq_bool(py, obj_from_bits(value), item);
        dec_ref_bits(py, value);
        match outcome {
            crate::object::ops_compare::CompareBoolOutcome::True => {
                return MoltObject::from_bool(true).bits();
            }
            crate::object::ops_compare::CompareBoolOutcome::Error => {
                return MoltObject::none().bits();
            }
            crate::object::ops_compare::CompareBoolOutcome::False
            | crate::object::ops_compare::CompareBoolOutcome::NotComparable => {}
        }
        index += 1;
    }
}
fn iterable_contains(_py: &PyToken<'_>, container_bits: u64, item_bits: u64) -> u64 {
    let item = obj_from_bits(item_bits);
    // OwnedIterator already owns __iter__/__getitem__ fallback,
    // item custody, exhaustion and errors. Membership adds only
    // identity-or-rich-equality and releases each owned item.
    let Some(mut iter) = crate::object::iterable::OwnedIterator::new(_py, container_bits) else {
        return MoltObject::none().bits();
    };
    loop {
        let value = match iter.next() {
            Ok(Some(value)) => value,
            Ok(None) => return MoltObject::from_bool(false).bits(),
            Err(()) => return MoltObject::none().bits(),
        };
        let outcome =
            crate::object::ops_compare::compare_object_eq_bool(_py, obj_from_bits(value), item);
        dec_ref_bits(_py, value);
        match outcome {
            crate::object::ops_compare::CompareBoolOutcome::True => {
                return MoltObject::from_bool(true).bits();
            }
            crate::object::ops_compare::CompareBoolOutcome::Error => {
                return MoltObject::none().bits();
            }
            crate::object::ops_compare::CompareBoolOutcome::False
            | crate::object::ops_compare::CompareBoolOutcome::NotComparable => {}
        }
    }
}

fn contains_impl(container_bits: u64, item_bits: u64, builtin_only: bool) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let container = obj_from_bits(container_bits);
        let item = obj_from_bits(item_bits);
        if let Some(ptr) = container.as_ptr() {
            unsafe {
                if !builtin_only && !crate::object::iterable::builtin_receiver(_py, ptr) {
                    if let Some(method) = crate::builtins::attr::lookup_special_method(
                        _py,
                        container_bits,
                        b"__contains__",
                    ) {
                        let result = call_callable1(_py, method, item_bits);
                        dec_ref_bits(_py, method);
                        if exception_pending(_py) {
                            dec_ref_bits(_py, result);
                            return MoltObject::none().bits();
                        }
                        let truth = is_truthy(_py, obj_from_bits(result));
                        dec_ref_bits(_py, result);
                        return if exception_pending(_py) {
                            MoltObject::none().bits()
                        } else {
                            MoltObject::from_bool(truth).bits()
                        };
                    }
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                }
                let type_id = object_type_id(ptr);
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    let found = dict_find_entry(_py, dict_ptr, item_bits);
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_bool(found.is_some()).bits();
                }
                match type_id {
                    TYPE_ID_LIST | TYPE_ID_LIST_INT | TYPE_ID_LIST_BOOL | TYPE_ID_TUPLE => {
                        return sequence_contains_builtin(_py, ptr, item_bits);
                    }
                    TYPE_ID_DICT_KEYS_VIEW | TYPE_ID_DICT_ITEMS_VIEW => {
                        use crate::object::ops_compare::builtin_families::BuiltinComparison;
                        let family = if type_id == TYPE_ID_DICT_KEYS_VIEW {
                            BuiltinComparison::DictKeys
                        } else {
                            BuiltinComparison::DictItems
                        };
                        return family.invoke_contains(_py, container_bits, item_bits);
                    }
                    TYPE_ID_SET | TYPE_ID_FROZENSET => {
                        return crate::object::ops_set::set_contains(
                            _py,
                            container_bits,
                            item_bits,
                            crate::object::ops_set::SetContainsPolicy::Python,
                        );
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
                        return crate::object::ops_bytes::bytes_contains_builtin(
                            _py,
                            container_bits,
                            item_bits,
                        );
                    }
                    TYPE_ID_RANGE => {
                        if let Some(candidate) = range_lookup_candidate(_py, item_bits) {
                            let Some((start, stop, step)) = range_components_bigint(ptr) else {
                                return MoltObject::none().bits();
                            };
                            return MoltObject::from_bool(
                                range_index_for_candidate(&start, &stop, &step, &candidate)
                                    .is_some(),
                            )
                            .bits();
                        }
                        return iterable_contains(_py, container_bits, item_bits);
                    }
                    _ => {}
                }
                if let Some(call_bits) = crate::builtins::attr::lookup_special_method(
                    _py,
                    container_bits,
                    b"__contains__",
                ) {
                    let result = call_callable1(_py, call_bits, item_bits);
                    dec_ref_bits(_py, call_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, result);
                        return MoltObject::none().bits();
                    }
                    let truth = is_truthy(_py, obj_from_bits(result));
                    dec_ref_bits(_py, result);
                    return if exception_pending(_py) {
                        MoltObject::none().bits()
                    } else {
                        MoltObject::from_bool(truth).bits()
                    };
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
                return iterable_contains(_py, container_bits, item_bits);
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
                if object_type_id(ptr) != TYPE_ID_LIST
                    || !crate::object::iterable::builtin_receiver(_py, ptr)
                {
                    return molt_contains(container_bits, item_bits);
                }
                return sequence_contains_builtin(_py, ptr, item_bits);
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
