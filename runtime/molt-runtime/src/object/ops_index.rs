//! Index, store, delete, contains, and unpack operations.

use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};
use std::collections::HashMap;


#[unsafe(no_mangle)]
pub extern "C" fn molt_index(obj_bits: u64, key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Fast path: dict[key] — skips exception_pending and type dispatch chain.
        if let Some(obj_ptr) = obj_from_bits(obj_bits).as_ptr() {
            unsafe {
                if object_type_id(obj_ptr) == TYPE_ID_DICT {
                    if let Some(val) = dict_get_in_place(_py, obj_ptr, key_bits) {
                        if obj_from_bits(val).as_ptr().is_some() {
                            inc_ref_bits(_py, val);
                        }
                        return val;
                    }
                    return raise_key_error_with_key(_py, key_bits);
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
                    let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                        Some(fmt) => fmt,
                        None => return MoltObject::none().bits(),
                    };
                    let owner_bits = memoryview_owner_bits(ptr);
                    let owner = obj_from_bits(owner_bits);
                    let owner_ptr = match owner.as_ptr() {
                        Some(ptr) => ptr,
                        None => return MoltObject::none().bits(),
                    };
                    let base = match bytes_like_slice_raw(owner_ptr) {
                        Some(slice) => slice,
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
                                let val =
                                    memoryview_read_scalar(_py, base, memoryview_offset(ptr), fmt);
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
                        let mut pos = memoryview_offset(ptr);
                        for (dim, &elem_bits) in elems.iter().enumerate() {
                            let Some(idx) = index_i64_with_overflow(
                                _py,
                                elem_bits,
                                "memoryview: invalid slice key",
                                None,
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
                        if pos < 0 || pos + fmt.itemsize as isize > base.len() as isize {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "index out of bounds on dimension 1",
                            );
                        }
                        let val = memoryview_read_scalar(_py, base, pos, fmt);
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
                        let out_ptr = alloc_memoryview_shaped(
                            _py,
                            memoryview_owner_bits(ptr),
                            new_offset,
                            itemsize,
                            memoryview_readonly(ptr),
                            memoryview_format_bits(ptr),
                            new_shape,
                            new_strides,
                        );
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
                    let Some(idx) = index_i64_with_overflow(
                        _py,
                        key_bits,
                        "memoryview: invalid slice key",
                        None,
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
                    let pos = memoryview_offset(ptr) + (i as isize) * strides[0];
                    if pos < 0 || pos + fmt.itemsize as isize > base.len() as isize {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "index out of bounds on dimension 1",
                        );
                    }
                    let val = memoryview_read_scalar(_py, base, pos, fmt);
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
                    let type_err = if type_id == TYPE_ID_STRING {
                        format!(
                            "string indices must be integers, not '{}'",
                            type_name(_py, key)
                        )
                    } else if type_id == TYPE_ID_BYTES {
                        format!(
                            "byte indices must be integers or slices, not {}",
                            type_name(_py, key)
                        )
                    } else {
                        format!(
                            "bytearray indices must be integers or slices, not {}",
                            type_name(_py, key)
                        )
                    };
                    let Some(idx) = index_i64_with_overflow(_py, key_bits, &type_err, None) else {
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
                    let idx = if let Some(i) = to_i64(key) {
                        i
                    } else {
                        let key_type = type_name(_py, key);
                        if debug_index_enabled() {
                            eprintln!(
                                "molt index type-error op=get container=list key_type={} key_bits=0x{:x} key_float={:?}",
                                key_type,
                                key_bits,
                                key.as_float()
                            );
                        }
                        let type_err =
                            format!("list indices must be integers or slices, not {}", key_type);
                        let Some(i) = index_i64_with_overflow(_py, key_bits, &type_err, None)
                        else {
                            return MoltObject::none().bits();
                        };
                        i
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
                    let idx = if let Some(i) = to_i64(key) {
                        i
                    } else {
                        let key_type = type_name(_py, key);
                        if debug_index_enabled() {
                            eprintln!(
                                "molt index type-error op=get container=tuple key_type={} key_bits=0x{:x} key_float={:?}",
                                key_type,
                                key_bits,
                                key.as_float()
                            );
                        }
                        let type_err =
                            format!("tuple indices must be integers or slices, not {}", key_type);
                        let Some(i) = index_i64_with_overflow(_py, key_bits, &type_err, None)
                        else {
                            return MoltObject::none().bits();
                        };
                        i
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
                    if let Some((start_i64, stop_i64, step_i64)) = range_components_i64(ptr)
                        && let Some(mut idx_i64) = to_i64(key)
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
                    let type_err = format!(
                        "range indices must be integers or slices, not {}",
                        type_name(_py, key)
                    );
                    let Some(mut idx) = index_bigint_from_obj(_py, key_bits, &type_err) else {
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
                    if object_type_id(ptr) != TYPE_ID_DICT
                        && let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__missing__")
                    {
                        if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, ptr, name_bits)
                        {
                            dec_ref_bits(_py, name_bits);
                            let res = call_callable1(_py, call_bits, key_bits);
                            dec_ref_bits(_py, call_bits);
                            if exception_pending(_py) {
                                return MoltObject::none().bits();
                            }
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
                    if let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__class_getitem__")
                    {
                        if let Some(call_bits) = class_attr_lookup(_py, ptr, ptr, Some(ptr), name_bits)
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
                    // Default __class_getitem__: create a GenericAlias
                    // directly.  This matches CPython >= 3.12 where every
                    // type supports subscript via a default that returns
                    // types.GenericAlias(cls, params).
                    return crate::builtins::types::molt_generic_alias_new(obj_bits, key_bits);
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
                eprintln!("[MOLT-DEBUG] subscript fail (TYPE_ID_TYPE, no __class_getitem__): class_name={}, obj_bits=0x{:016x}, key_bits=0x{:016x}", class_name, obj_bits, key_bits);
                format!("type '{}' is not subscriptable", class_name)
            } else {
                let tn = type_name(_py, obj);
                let tid = unsafe { object_type_id(ptr) };
                eprintln!("[MOLT-DEBUG] subscript fail (ptr path): type_name={}, type_id={}, obj_bits=0x{:016x}, key_bits=0x{:016x}", tn, tid, obj_bits, key_bits);
                format!("'{}' object is not subscriptable", tn)
            };
            return raise_exception::<_>(_py, "TypeError", &msg);
        }
        let obj_dbg = obj_from_bits(obj_bits);
        eprintln!("[MOLT-DEBUG] subscript fail (no-ptr path): type_name={}, obj_bits=0x{:016x}, key_bits=0x{:016x}, is_int={}, is_float={}, is_bool={}, is_none={}, is_pending={}", type_name(_py, obj_dbg), obj_bits, key_bits, obj_dbg.is_int(), obj_dbg.is_float(), obj_dbg.is_bool(), obj_dbg.is_none(), obj_dbg.is_pending());
        let msg = format!("'{}' object is not subscriptable", type_name(_py, obj));
        raise_exception::<_>(_py, "TypeError", &msg)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_store_index(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
            }
        }
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
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
                            for &item in new_items.iter() {
                                inc_ref_bits(_py, item);
                            }
                            let removed: Vec<u64> =
                                elems.splice(s..e, new_items.iter().copied()).collect();
                            for old_bits in removed {
                                dec_ref_bits(_py, old_bits);
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
                        for &item in new_items.iter() {
                            inc_ref_bits(_py, item);
                        }
                        for (idx, &item) in indices.iter().zip(new_items.iter()) {
                            let old_bits = elems[*idx];
                            if old_bits != item {
                                dec_ref_bits(_py, old_bits);
                                elems[*idx] = item;
                            }
                        }
                        return obj_bits;
                    }
                    let idx = if let Some(i) = to_i64(key) {
                        i
                    } else {
                        let key_type = type_name(_py, key);
                        if debug_index_enabled() {
                            eprintln!(
                                "molt index type-error op=set container=list key_type={} key_bits=0x{:x} key_float={:?}",
                                key_type,
                                key_bits,
                                key.as_float()
                            );
                        }
                        let type_err =
                            format!("list indices must be integers or slices, not {}", key_type);
                        let Some(i) = index_i64_with_overflow(_py, key_bits, &type_err, None)
                        else {
                            return MoltObject::none().bits();
                        };
                        i
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
                        dec_ref_bits(_py, old_bits);
                        inc_ref_bits(_py, val_bits);
                        elems[i as usize] = val_bits;
                    }
                    return obj_bits;
                }
                if type_id == TYPE_ID_TUPLE {
                    return MoltObject::none().bits();
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
                    let type_err = format!(
                        "bytearray indices must be integers or slices, not {}",
                        type_name(_py, key)
                    );
                    let Some(idx) = index_i64_with_overflow(_py, key_bits, &type_err, None) else {
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
                    if memoryview_readonly(ptr) {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "cannot modify read-only memory",
                        );
                    }
                    let owner_bits = memoryview_owner_bits(ptr);
                    let owner = obj_from_bits(owner_bits);
                    let owner_ptr = match owner.as_ptr() {
                        Some(ptr) => ptr,
                        None => return MoltObject::none().bits(),
                    };
                    if object_type_id(owner_ptr) != TYPE_ID_BYTEARRAY {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "memoryview is not writable",
                        );
                    }
                    let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                        Some(fmt) => fmt,
                        None => return MoltObject::none().bits(),
                    };
                    let shape = memoryview_shape(ptr).unwrap_or(&[]);
                    let strides = memoryview_strides(ptr).unwrap_or(&[]);
                    let ndim = shape.len();
                    let data = bytearray_vec(owner_ptr);
                    if ndim == 0 {
                        if let Some(tup_ptr) = key.as_ptr()
                            && object_type_id(tup_ptr) == TYPE_ID_TUPLE
                        {
                            let elems = seq_vec_ref(tup_ptr);
                            if elems.is_empty() {
                                let ok = memoryview_write_scalar(
                                    _py,
                                    data.as_mut_slice(),
                                    memoryview_offset(ptr),
                                    fmt,
                                    val_bits,
                                );
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
                        let mut pos = memoryview_offset(ptr);
                        for (dim, &elem_bits) in elems.iter().enumerate() {
                            let Some(idx) = index_i64_with_overflow(
                                _py,
                                elem_bits,
                                "memoryview: invalid slice key",
                                None,
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
                        if pos < 0 || pos + fmt.itemsize as isize > data.len() as isize {
                            return raise_exception::<_>(
                                _py,
                                "IndexError",
                                "index out of bounds on dimension 1",
                            );
                        }
                        let ok =
                            memoryview_write_scalar(_py, data.as_mut_slice(), pos, fmt, val_bits);
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
                            if pos < 0 || pos + fmt.itemsize as isize > data.len() as isize {
                                return MoltObject::none().bits();
                            }
                            let start = pos as usize;
                            let end = start + fmt.itemsize;
                            data[start..end].copy_from_slice(&src_bytes[idx..idx + fmt.itemsize]);
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
                    let Some(idx) = index_i64_with_overflow(
                        _py,
                        key_bits,
                        "memoryview: invalid slice key",
                        None,
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
                    let pos = memoryview_offset(ptr) + (i as isize) * strides[0];
                    if pos < 0 || pos + fmt.itemsize as isize > data.len() as isize {
                        return raise_exception::<_>(
                            _py,
                            "IndexError",
                            "index out of bounds on dimension 1",
                        );
                    }
                    let ok = memoryview_write_scalar(_py, data.as_mut_slice(), pos, fmt, val_bits);
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
    crate::with_gil_entry!(_py, {
        let obj = obj_from_bits(obj_bits);
        let key = obj_from_bits(key_bits);
        if let Some(ptr) = obj.as_ptr() {
            unsafe {
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
                        if step > 0 {
                            for &idx in indices.iter().rev() {
                                let old_bits = elems.remove(idx);
                                dec_ref_bits(_py, old_bits);
                            }
                        } else {
                            for &idx in indices.iter() {
                                let old_bits = elems.remove(idx);
                                dec_ref_bits(_py, old_bits);
                            }
                        }
                        return obj_bits;
                    }
                    let key_type = type_name(_py, key);
                    if debug_index_enabled() {
                        eprintln!(
                            "molt index type-error op=del container=list key_type={} key_bits=0x{:x} key_float={:?}",
                            key_type,
                            key_bits,
                            key.as_float()
                        );
                    }
                    let type_err =
                        format!("list indices must be integers or slices, not {}", key_type);
                    let Some(idx) = index_i64_with_overflow(_py, key_bits, &type_err, None) else {
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
                    let type_err = format!(
                        "bytearray indices must be integers or slices, not {}",
                        type_name(_py, key)
                    );
                    let Some(idx) = index_i64_with_overflow(_py, key_bits, &type_err, None) else {
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
    crate::with_gil_entry!(_py, { molt_index(obj_bits, key_bits) })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_setitem_method(obj_bits: u64, key_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        let _ = molt_del_index(obj_bits, key_bits);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_contains(container_bits: u64, item_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let container = obj_from_bits(container_bits);
        let item = obj_from_bits(item_bits);
        if let Some(ptr) = container.as_ptr() {
            unsafe {
                let type_id = object_type_id(ptr);
                if let Some(dict_bits) = dict_like_bits_from_ptr(_py, ptr) {
                    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                        return MoltObject::none().bits();
                    };
                    if !ensure_hashable(_py, item_bits) {
                        return MoltObject::none().bits();
                    }
                    let order = dict_order(dict_ptr);
                    let table = dict_table(dict_ptr);
                    let found = dict_find_entry(_py, order, table, item_bits);
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
                        if !ensure_hashable(_py, item_bits) {
                            return MoltObject::none().bits();
                        }
                        let order = set_order(ptr);
                        let table = set_table(ptr);
                        let found = set_find_entry(_py, order, table, item_bits);
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
                        let candidate = if let Some(f) = item.as_float() {
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
                        let owner_bits = memoryview_owner_bits(ptr);
                        let owner = obj_from_bits(owner_bits);
                        let owner_ptr = match owner.as_ptr() {
                            Some(ptr) => ptr,
                            None => {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    &format!(
                                        "a bytes-like object is required, not '{}'",
                                        type_name(_py, item)
                                    ),
                                );
                            }
                        };
                        let base = match bytes_like_slice_raw(owner_ptr) {
                            Some(slice) => slice,
                            None => {
                                return raise_exception::<_>(
                                    _py,
                                    "TypeError",
                                    &format!(
                                        "a bytes-like object is required, not '{}'",
                                        type_name(_py, item)
                                    ),
                                );
                            }
                        };
                        let offset = memoryview_offset(ptr);
                        let len = memoryview_len(ptr);
                        let itemsize = memoryview_itemsize(ptr);
                        let stride = memoryview_stride(ptr);
                        if offset < 0 {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                &format!(
                                    "a bytes-like object is required, not '{}'",
                                    type_name(_py, item)
                                ),
                            );
                        }
                        if itemsize != 1 {
                            return raise_exception::<_>(
                                _py,
                                "TypeError",
                                "memoryview itemsize not supported",
                            );
                        }
                        if stride == 1 {
                            let start = offset as usize;
                            let end = start.saturating_add(len);
                            let hay = &base[start.min(base.len())..end.min(base.len())];
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
                            let start = offset + (idx as isize) * stride;
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
                            let start = start as usize;
                            if start >= base.len() {
                                break;
                            }
                            out.push(base[start]);
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
                                dec_ref_bits(_py, exc_bits);
                                exception_stack_pop(_py);
                                if is_index_error {
                                    clear_exception(_py);
                                    return MoltObject::from_bool(false).bits();
                                }
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
    crate::with_gil_entry!(_py, {
        let container = obj_from_bits(container_bits);
        if let Some(ptr) = container.as_ptr() {
            unsafe {
                let mut idx = 0usize;
                while let Some(val) = list_elem_at(ptr, idx) {
                    let elem_bits = val;
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
    crate::with_gil_entry!(_py, {
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

pub(crate) extern "C" fn dict_pop_method(
    self_bits: u64,
    key_bits: u64,
    default_bits: u64,
    has_default_bits: u64,
) -> i64 {
    molt_dict_pop(self_bits, key_bits, default_bits, has_default_bits) as i64
}

pub(crate) extern "C" fn dict_clear_method(self_bits: u64) -> i64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
            dec_ref_bits(_py, key_bits);
            dec_ref_bits(_py, val_bits);
            order.truncate(order.len() - 2);
            let entries = order.len() / 2;
            let table = dict_table(ptr);
            let capacity = dict_table_capacity(entries.max(1));
            dict_rebuild(_py, order, table, capacity);
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
        if !ensure_hashable(_py, key_bits) {
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
            dict_rebuild(_py, order, table, capacity);
            if exception_pending(_py) {
                return Some(false);
            }
        }
        order.push(key_bits);
        order.push(sum_bits);
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
        let table = dict_table(dict_ptr);
        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, table, capacity);
            if exception_pending(_py) {
                if sum_owned {
                    dec_ref_bits(_py, sum_bits);
                }
                dec_ref_bits(_py, key_bits);
                return false;
            }
        }
        order.push(key_bits);
        order.push(sum_bits);
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
        let table = dict_table(dict_ptr);
        let new_entries = (order.len() / 2) + 1;
        let needs_resize = table.is_empty() || new_entries * 10 >= table.len() * 7;
        if needs_resize {
            let capacity = dict_table_capacity(new_entries);
            dict_rebuild(_py, order, table, capacity);
            if exception_pending(_py) {
                dec_ref_bits(_py, default_bits);
                dec_ref_bits(_py, key_bits);
                return None;
            }
        }
        order.push(key_bits);
        order.push(default_bits);
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

/// Outlined sequence unpacking helper. Validates that the sequence length
/// matches `expected_count`, extracts each element (with incref), and writes
/// element bits to `output_ptr[0..expected_count]`.
///
/// Returns 0 on success.  On length mismatch a `ValueError` is raised through
/// the normal exception-pending mechanism and `MoltObject::none().bits()` is
/// returned so the caller can short-circuit.
#[unsafe(no_mangle)]
pub extern "C" fn molt_unpack_sequence(
    seq_bits: u64,
    expected_count: u64,
    output_ptr: *mut u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let obj = obj_from_bits(seq_bits);
        let expected = expected_count as usize;
        let Some(ptr) = obj.as_ptr() else {
            raise_exception::<u64>(
                _py,
                "TypeError",
                "cannot unpack non-sequence",
            );
            return MoltObject::none().bits();
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                let elems: &[u64] = seq_vec_ref(ptr);
                let actual = elems.len();
                if actual < expected {
                    let msg = format!(
                        "not enough values to unpack (expected {}, got {})",
                        expected, actual
                    );
                    raise_exception::<u64>(_py, "ValueError", &msg);
                    return MoltObject::none().bits();
                }
                if actual > expected {
                    let msg = format!(
                        "too many values to unpack (expected {})",
                        expected
                    );
                    raise_exception::<u64>(_py, "ValueError", &msg);
                    return MoltObject::none().bits();
                }
                let out_slice = std::slice::from_raw_parts_mut(output_ptr, expected);
                for (i, &bits) in elems.iter().enumerate().take(expected) {
                    inc_ref_bits(_py, bits);
                    out_slice[i] = bits;
                }
            } else {
                // Generic iterable: materialize via iter/next.
                let iter_bits = molt_iter(seq_bits);
                if obj_from_bits(iter_bits).is_none() {
                    raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "cannot unpack non-sequence",
                    );
                    return MoltObject::none().bits();
                }
                let out_slice = std::slice::from_raw_parts_mut(output_ptr, expected);
                let mut count = 0usize;
                loop {
                    let pair_bits = molt_iter_next(iter_bits);
                    let pair_obj = obj_from_bits(pair_bits);
                    let Some(pair_ptr) = pair_obj.as_ptr() else {
                        break;
                    };
                    if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                        break;
                    }
                    let pair_elems = seq_vec_ref(pair_ptr);
                    if pair_elems.len() < 2 {
                        break;
                    }
                    let done = is_truthy(_py, obj_from_bits(pair_elems[1]));
                    if done {
                        break;
                    }
                    let val_bits = pair_elems[0];
                    if count < expected {
                        inc_ref_bits(_py, val_bits);
                        out_slice[count] = val_bits;
                    }
                    count += 1;
                    if count > expected {
                        dec_ref_bits(_py, iter_bits);
                        let msg = format!(
                            "too many values to unpack (expected {})",
                            expected
                        );
                        raise_exception::<u64>(_py, "ValueError", &msg);
                        return MoltObject::none().bits();
                    }
                }
                dec_ref_bits(_py, iter_bits);
                if count < expected {
                    let msg = format!(
                        "not enough values to unpack (expected {}, got {})",
                        expected, count
                    );
                    raise_exception::<u64>(_py, "ValueError", &msg);
                    // Dec-ref any already-extracted values.
                    for i in 0..count {
                        dec_ref_bits(_py, out_slice[i]);
                    }
                    return MoltObject::none().bits();
                }
            }
        }
        0u64
    })
}
