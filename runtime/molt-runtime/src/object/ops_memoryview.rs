//! Memoryview and buffer operations.

use super::ops::{eq_bool_from_bits, is_truthy, type_name};
use super::ops_bytes::bytes_hex_from_bits;
use crate::builtins::compatibility_error::CompatibilityError;
use crate::object::memoryview::{
    memoryview_adjust_format, memoryview_checked_nbytes, memoryview_prepare_iter,
    memoryview_read_item_at,
};
use crate::*;
use molt_obj_model::MoltObject;
use num_integer::Integer;

fn raise_memoryview_buffer_error(py: &PyToken<'_>, object: MoltObject) -> u64 {
    raise_exception(
        py,
        "TypeError",
        &format!(
            "memoryview: a bytes-like object is required, not '{}'",
            type_name(py, object)
        ),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_new(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                return raise_memoryview_buffer_error(_py, obj);
            }
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_MEMORYVIEW {
                if !super::memoryview::require_exportable(_py, ptr) {
                    return MoltObject::none().bits();
                }
                let storage = match TypedStridedStorage::from_object_bits(bits) {
                    Ok(storage) => storage,
                    Err(TypedStridedStorageError::ReleasedMemoryView) => {
                        return raise_released_memoryview(_py);
                    }
                    Err(_) => return MoltObject::none().bits(),
                };
                let out_ptr = alloc_memoryview_from_storage(_py, storage);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }

            if type_id == TYPE_ID_FOREIGN {
                let pointer = std::ptr::with_exposed_provenance_mut::<
                    molt_cpython_abi::abi_types::PyObject,
                >(crate::object::foreign::foreign_ptr_from_obj(ptr));
                if pointer.is_null() {
                    return raise_exception(_py, "TypeError", "invalid native buffer exporter");
                }
                let lease = match molt_cpython_abi::api::memory::MemoryViewLease::acquire(
                    pointer,
                    molt_cpython_abi::abi_types::PyBUF_FULL_RO,
                    molt_cpython_abi::api::buffer::PyObject_GetBuffer,
                ) {
                    Ok(lease) => lease,
                    Err(()) => {
                        crate::cpython_abi_hooks::propagate_native_failure(
                            _py,
                            "native memoryview acquisition",
                        );
                        return MoltObject::none().bits();
                    }
                };
                let descriptor = match molt_cpython_abi::api::buffer::descriptor_from_pybuffer(
                    lease.descriptor(),
                ) {
                    Ok(descriptor) => descriptor,
                    Err(()) => {
                        return raise_exception(
                            _py,
                            "BufferError",
                            "invalid or indirect memoryview buffer descriptor",
                        );
                    }
                };
                let format = (*lease.descriptor()).format;
                return super::memoryview::from_native_descriptor(
                    _py,
                    &descriptor,
                    format,
                    Some(lease),
                );
            }
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let readonly = type_id == TYPE_ID_BYTES;
                let format_ptr = alloc_string(_py, b"B");
                if format_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let format_bits = MoltObject::from_ptr(format_ptr).bits();
                // Allocation can run finalizers; capture byte length together
                // with the data pointer only after that callback boundary.
                let len = bytes_len(ptr);
                let storage = TypedStridedStorage::one_dim(
                    bytes_data(ptr) as *mut u8,
                    readonly,
                    len,
                    1,
                    1,
                    0,
                    bits,
                    format_bits,
                );
                let out_ptr = match storage {
                    Some(storage) => alloc_memoryview_from_storage(_py, storage),
                    None => std::ptr::null_mut(),
                };
                dec_ref_bits(_py, format_bits);
                if out_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(out_ptr).bits();
            }
            if type_id == TYPE_ID_OBJECT || type_id == TYPE_ID_NATIVE_HANDLE {
                let mut exported = MoltBufferView::default();
                if crate::c_api::molt_buffer_acquire(bits, &mut exported) == 0 {
                    let out = crate::c_api::molt_memoryview_from_buffer(&exported);
                    crate::c_api::molt_buffer_release(&mut exported);
                    return out;
                }
                if exception_pending(_py) {
                    return MoltObject::none().bits();
                }
            }
        }
        raise_memoryview_buffer_error(_py, obj)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_from_flags(obj_bits: u64, flags_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let flag_type = class_name_for_error(type_of_bits(_py, flags_bits));
        let err = format!("'{flag_type}' object cannot be interpreted as an integer");
        let Some(flags) = index_bigint_from_obj(_py, flags_bits, &err) else {
            return MoltObject::none().bits();
        };
        if flags.is_odd()
            && let Some(obj_ptr) = maybe_ptr_from_bits(obj_bits)
        {
            unsafe {
                let type_id = object_type_id(obj_ptr);
                // CPython ignores writable-flag checks when the input is already a memoryview.
                if type_id == TYPE_ID_BYTES {
                    return raise_exception::<_>(_py, "BufferError", "Object is not writable.");
                }
            }
        }
        molt_memoryview_new(obj_bits)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_cast(
    view_bits: u64,
    format_bits: u64,
    shape_bits: u64,
    has_shape_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let view = obj_from_bits(view_bits);
        let view_ptr = match view.as_ptr() {
            Some(ptr) => ptr,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cast() argument 'view' must be a memoryview",
                );
            }
        };
        unsafe {
            if object_type_id(view_ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "cast() argument 'view' must be a memoryview",
                );
            }
            if !super::memoryview::require_exportable(_py, view_ptr) {
                return MoltObject::none().bits();
            }
            if memoryview_released(view_ptr) {
                return raise_released_memoryview(_py);
            }
            let format_obj = obj_from_bits(format_bits);
            let format_str = match string_obj_to_owned(format_obj) {
                Some(val) => val,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!(
                            "cast() argument 'format' must be str, not {}",
                            type_name(_py, format_obj)
                        ),
                    );
                }
            };
            let fmt = match memoryview_format_from_str(&format_str) {
                Some(val) => val,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "memoryview: destination format must be a native single character format prefixed with an optional '@'",
                    );
                }
            };
            if !memoryview_is_c_contiguous_view(view_ptr) {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "memoryview: casts are restricted to C-contiguous views",
                );
            }
            let shape_view = memoryview_shape(view_ptr).unwrap_or(&[]);
            let nbytes = match memoryview_checked_nbytes(shape_view, memoryview_itemsize(view_ptr))
            {
                Some(val) => val,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "BufferError",
                        "invalid memoryview cast storage",
                    );
                }
            };
            let has_shape = is_truthy(_py, obj_from_bits(has_shape_bits));
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if memoryview_released(view_ptr) {
                return raise_released_memoryview(_py);
            }
            let shape = if has_shape {
                let shape_obj = obj_from_bits(shape_bits);
                let shape_ptr = match shape_obj.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "shape must be a list or a tuple",
                        );
                    }
                };
                let type_id = object_type_id(shape_ptr);
                if type_id != TYPE_ID_LIST && type_id != TYPE_ID_TUPLE {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "shape must be a list or a tuple",
                    );
                }
                let Some(elems) = crate::object::seq_access::snapshot(
                    _py,
                    shape_ptr,
                    "memoryview shape snapshot allocation failed",
                ) else {
                    return MoltObject::none().bits();
                };
                if elems.len() > crate::object::memoryview::MOLT_BUFFER_MAX_NDIM {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "memoryview: number of dimensions must not exceed 64",
                    );
                }
                let mut shape = Vec::with_capacity(elems.len());
                for &elem_bits in elems.iter() {
                    let Some(val) = crate::builtins::numbers::index_i64_integral_bits(elem_bits)
                        .and_then(|value| isize::try_from(value).ok())
                    else {
                        if crate::builtins::numbers::index_bigint_integral_bits(elem_bits).is_some()
                        {
                            return raise_exception::<_>(
                                _py,
                                "OverflowError",
                                "Python int too large to convert to C ssize_t",
                            );
                        }
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "memoryview.cast(): elements of shape must be integers",
                        );
                    };
                    if val <= 0 {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "memoryview.cast(): elements of shape must be integers > 0",
                        );
                    }
                    shape.push(val);
                    // Match copy_shape's left-to-right overflow precedence;
                    // all storage consumers share the same byte-extent limit.
                    if memoryview_checked_nbytes(&shape, fmt.itemsize).is_none() {
                        return raise_exception::<_>(
                            _py,
                            "ValueError",
                            "memoryview.cast(): product(shape) > SSIZE_MAX",
                        );
                    }
                }
                shape
            } else {
                let itemsize = fmt.itemsize;
                if itemsize == 0 || nbytes % itemsize != 0 {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "memoryview: length is not a multiple of itemsize",
                    );
                }
                let len = (nbytes / itemsize) as isize;
                vec![len]
            };
            let Some(byte_len) = memoryview_checked_nbytes(&shape, fmt.itemsize) else {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "memoryview.cast(): product(shape) > SSIZE_MAX",
                );
            };
            if byte_len != nbytes {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "memoryview: product(shape) * itemsize != buffer size",
                );
            }
            let mut strides = vec![0isize; shape.len()];
            let mut stride = fmt.itemsize as isize;
            for idx in (0..shape.len()).rev() {
                strides[idx] = stride;
                let Some(next_stride) = stride.checked_mul(shape[idx].max(1)) else {
                    return raise_exception::<_>(
                        _py,
                        "BufferError",
                        "invalid memoryview cast strides",
                    );
                };
                stride = next_stride;
            }
            let data = memoryview_data(view_ptr);
            if data.is_null() {
                return raise_exception::<_>(_py, "BufferError", "invalid memoryview cast storage");
            }
            let storage = TypedStridedStorage::new(
                data,
                memoryview_readonly(view_ptr),
                fmt.itemsize,
                memoryview_offset(view_ptr),
                memoryview_base_bits(view_ptr),
                format_bits,
                shape,
                strides,
            )
            .map(|storage| {
                storage
                    .with_owner(memoryview_owner_bits(view_ptr))
                    .with_native_lease((*memoryview_ptr(view_ptr)).native_lease.clone())
            });
            let out_ptr = match storage {
                Some(storage) => alloc_memoryview_from_storage(_py, storage),
                None => {
                    return raise_exception::<_>(
                        _py,
                        "BufferError",
                        "invalid memoryview cast storage",
                    );
                }
            };
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

pub(crate) extern "C" fn memoryview_tobytes_method(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) =
            crate::builtins::native_arguments::NativeArguments::read(py, "tobytes", args, kwargs)
        else {
            return MoltObject::none().bits();
        };
        let Some(&receiver) = call.positional.first() else {
            return raise_exception::<_>(
                py,
                "TypeError",
                "unbound method memoryview.tobytes() needs an argument",
            );
        };
        if !obj_from_bits(receiver)
            .as_ptr()
            .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_MEMORYVIEW })
        {
            return raise_exception::<_>(
                py,
                "TypeError",
                &format!(
                    "descriptor 'tobytes' for 'memoryview' objects doesn't apply to a '{}' object",
                    type_name(py, obj_from_bits(receiver)),
                ),
            );
        }
        let Some(bound) = crate::builtins::native_arguments::bind_named(
            py,
            "tobytes",
            call.values(),
            call.vector_keyword_view(),
            ["order"],
            0,
        ) else {
            return MoltObject::none().bits();
        };
        let [order] = *bound;
        // Clinic converts its nullable string before the released-view check.
        let order = match order.filter(|&bits| !obj_from_bits(bits).is_none()) {
            None => None,
            Some(bits) => {
                if !obj_from_bits(bits)
                    .as_ptr()
                    .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
                {
                    return raise_exception::<_>(
                        py,
                        "TypeError",
                        &format!(
                            "tobytes() argument 'order' must be str or None, not {}",
                            type_name(py, obj_from_bits(bits)),
                        ),
                    );
                }
                if !crate::object::ops_string::require_strict_utf8(py, bits) {
                    return MoltObject::none().bits();
                }
                let value =
                    string_obj_to_owned(obj_from_bits(bits)).expect("admitted Unicode order");
                if value.as_bytes().contains(&0) {
                    return raise_exception::<_>(py, "ValueError", "embedded null character");
                }
                Some(value)
            }
        };
        memoryview_tobytes_ordered(py, receiver, order.as_deref())
    })
}

fn memoryview_tobytes_ordered(py: &PyToken<'_>, bits: u64, order: Option<&str>) -> u64 {
    let Some(ptr) = obj_from_bits(bits)
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_MEMORYVIEW })
    else {
        return raise_exception::<_>(py, "TypeError", "tobytes expects a memoryview");
    };
    unsafe {
        if memoryview_released(ptr) {
            return raise_released_memoryview(py);
        }
        use crate::object::memoryview::{MemoryViewOrder, memoryview_collect_bytes_in_order};
        let order = match order {
            None | Some("C") => MemoryViewOrder::C,
            Some("F") => MemoryViewOrder::Fortran,
            Some("A") => MemoryViewOrder::Any,
            _ => return raise_exception::<_>(py, "ValueError", "order must be 'C', 'F' or 'A'"),
        };
        let Some(out) = memoryview_collect_bytes_in_order(ptr, order) else {
            return MoltObject::none().bits();
        };
        let out_ptr = alloc_bytes(py, &out);
        if out_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_tobytes(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, { memoryview_tobytes_ordered(py, bits, None) })
}

unsafe fn memoryview_tolist_recursive(
    _py: &PyToken<'_>,
    view: *mut u8,
    format: &str,
    shape: &[isize],
    strides: &[isize],
    dim: usize,
    base_offset: isize,
) -> Option<u64> {
    if dim >= shape.len() || shape.len() != strides.len() {
        return None;
    }
    let dim_len = shape[dim].max(0) as usize;
    let mut items: Vec<u64> = Vec::with_capacity(dim_len);
    if dim + 1 == shape.len() {
        for i in 0..dim_len {
            let delta = memoryview_linear_offset(i, strides[dim])?;
            let item_offset = base_offset.checked_add(delta)?;
            let scalar = unsafe { memoryview_read_item_at(_py, view, item_offset, format) }?;
            items.push(scalar);
        }
    } else {
        for i in 0..dim_len {
            let delta = memoryview_linear_offset(i, strides[dim])?;
            let child_offset = base_offset.checked_add(delta)?;
            let child = unsafe {
                memoryview_tolist_recursive(
                    _py,
                    view,
                    format,
                    shape,
                    strides,
                    dim + 1,
                    child_offset,
                )
            }?;
            items.push(child);
        }
    }
    let out_ptr = alloc_list(_py, items.as_slice());
    if out_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(out_ptr).bits())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_tolist(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "tolist expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "tolist expects a memoryview");
            }
            if memoryview_released(ptr) {
                return raise_released_memoryview(_py);
            }
            let Some(format) = memoryview_adjust_format(_py, ptr) else {
                return MoltObject::none().bits();
            };
            let data = memoryview_data(ptr);
            if data.is_null() {
                return MoltObject::none().bits();
            }
            let shape = memoryview_shape(ptr).unwrap_or(&[]);
            let strides = memoryview_strides(ptr).unwrap_or(&[]);
            if shape.is_empty() || memoryview_ndim(ptr) == 0 {
                let scalar = match memoryview_read_item_at(_py, ptr, 0, &format) {
                    Some(bits) => bits,
                    None => return MoltObject::none().bits(),
                };
                return scalar;
            }
            match memoryview_tolist_recursive(_py, ptr, &format, shape, strides, 0, 0) {
                Some(bits) => bits,
                None => MoltObject::none().bits(),
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_count(bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "count expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "count expects a memoryview");
            }
            let Some(format) = memoryview_prepare_iter(_py, ptr) else {
                return MoltObject::none().bits();
            };
            let base = memoryview_data(ptr);
            if base.is_null() {
                return MoltObject::none().bits();
            }
            let len = memoryview_len(ptr);
            let stride = memoryview_stride(ptr);
            let mut count = 0i64;
            for idx in 0..len {
                let Some(item_offset) = memoryview_linear_offset(idx, stride) else {
                    return MoltObject::none().bits();
                };
                let Some(item_bits) = memoryview_read_item_at(_py, ptr, item_offset, &format)
                else {
                    return MoltObject::none().bits();
                };
                let eq = match eq_bool_from_bits(_py, item_bits, val_bits) {
                    Some(val) => val,
                    None => {
                        if obj_from_bits(item_bits).as_ptr().is_some() {
                            dec_ref_bits(_py, item_bits);
                        }
                        return MoltObject::none().bits();
                    }
                };
                if obj_from_bits(item_bits).as_ptr().is_some() {
                    dec_ref_bits(_py, item_bits);
                }
                if eq {
                    count += 1;
                }
            }
            MoltObject::from_int(count).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_index(bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "index expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "index expects a memoryview");
            }
            if memoryview_released(ptr) {
                return raise_released_memoryview(_py);
            }
            let ndim = memoryview_ndim(ptr);
            if ndim == 0 {
                return raise_exception::<_>(_py, "TypeError", "invalid lookup on 0-dim memory");
            }
            if ndim > 1 {
                return CompatibilityError::MemoryviewLookup { rank: ndim }.raise(_py);
            }
            let base = memoryview_data(ptr);
            if base.is_null() {
                return MoltObject::none().bits();
            }
            let len = memoryview_len(ptr);
            let stride = memoryview_stride(ptr);
            for idx in 0..len {
                let Some(format) = memoryview_adjust_format(_py, ptr) else {
                    return MoltObject::none().bits();
                };
                let Some(item_offset) = memoryview_linear_offset(idx, stride) else {
                    return MoltObject::none().bits();
                };
                let Some(item_bits) = memoryview_read_item_at(_py, ptr, item_offset, &format)
                else {
                    return MoltObject::none().bits();
                };
                let eq = match eq_bool_from_bits(_py, item_bits, val_bits) {
                    Some(val) => val,
                    None => {
                        if obj_from_bits(item_bits).as_ptr().is_some() {
                            dec_ref_bits(_py, item_bits);
                        }
                        return MoltObject::none().bits();
                    }
                };
                if obj_from_bits(item_bits).as_ptr().is_some() {
                    dec_ref_bits(_py, item_bits);
                }
                if eq {
                    return MoltObject::from_int(idx as i64).bits();
                }
            }
            raise_exception::<_>(_py, "ValueError", "memoryview.index(x): x not found")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_hex(bits: u64, sep_bits: u64, bytes_per_sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "hex expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "hex expects a memoryview");
            }
            if memoryview_released(ptr) {
                return raise_released_memoryview(_py);
            }
            let out = match memoryview_collect_bytes(ptr) {
                Some(out) => out,
                None => return MoltObject::none().bits(),
            };
            bytes_hex_from_bits(_py, out.as_slice(), sep_bits, bytes_per_sep_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_release(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "release expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "release expects a memoryview");
            }
            let view = &*memoryview_ptr(ptr);
            let exports = view.exports.count();
            if exports != 0 {
                return raise_exception::<u64>(
                    _py,
                    "BufferError",
                    &format!(
                        "memoryview has {exports} exported buffer{}",
                        if exports == 1 { "" } else { "s" }
                    ),
                );
            }
            let (owner, base, native) = super::buffer_exports::detach_memoryview_owner(ptr);
            drop(native);
            if owner != 0 {
                dec_ref_bits(_py, owner);
            }
            if base != 0 {
                dec_ref_bits(_py, base);
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_toreadonly(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                return raise_exception::<_>(_py, "TypeError", "toreadonly expects a memoryview");
            }
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "toreadonly expects a memoryview");
            }
            if !super::memoryview::require_exportable(_py, ptr) {
                return MoltObject::none().bits();
            }
            let storage = match TypedStridedStorage::from_object_bits(bits) {
                Ok(storage) => storage,
                Err(TypedStridedStorageError::ReleasedMemoryView) => {
                    return raise_released_memoryview(_py);
                }
                Err(_) => return MoltObject::none().bits(),
            };
            let out_ptr = alloc_memoryview_from_storage(_py, storage.with_readonly(true));
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

/// # Safety
/// Caller must ensure `out_ptr` is valid and writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_buffer_export(obj_bits: u64, out_ptr: *mut MoltBufferView) -> i32 {
    unsafe {
        crate::with_gil_entry_nopanic!(_py, {
            if out_ptr.is_null() {
                return 1;
            }
            let storage = match TypedStridedStorage::from_object_bits(obj_bits) {
                Ok(storage) => storage,
                Err(TypedStridedStorageError::ReleasedMemoryView) => {
                    let _ = raise_released_memoryview::<u64>(_py);
                    return 1;
                }
                // `array.array` exports its typed contiguous storage as a 1-D
                // buffer through the same typed-strided descriptor authority.
                Err(_) => {
                    match crate::builtins::array_mod::array_storage_from_object_bits(_py, obj_bits)
                    {
                        Ok(storage) => storage,
                        Err(_) => return 1,
                    }
                }
            };
            let Some(export) = MoltBufferView::from_typed_storage(&storage) else {
                return 1;
            };
            *out_ptr = export;
            0
        })
    }
}
