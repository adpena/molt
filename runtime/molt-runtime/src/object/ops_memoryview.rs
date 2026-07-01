//! Memoryview and buffer operations.

use super::ops::{eq_bool_from_bits, is_truthy, type_name};
use super::ops_bytes::bytes_hex_from_bits;
use crate::*;
use molt_obj_model::MoltObject;
use num_integer::Integer;
use num_traits::ToPrimitive;

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_new(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "memoryview expects a bytes-like object",
                );
            }
        };
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_MEMORYVIEW {
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
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let readonly = type_id == TYPE_ID_BYTES;
                let format_ptr = alloc_string(_py, b"B");
                if format_ptr.is_null() {
                    return MoltObject::none().bits();
                }
                let format_bits = MoltObject::from_ptr(format_ptr).bits();
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
        }
        raise_exception::<_>(_py, "TypeError", "memoryview expects a bytes-like object")
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
            let nbytes = match memoryview_nbytes_big(shape_view, memoryview_itemsize(view_ptr)) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            let has_shape = is_truthy(_py, obj_from_bits(has_shape_bits));
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
                let elems = seq_vec_ref(shape_ptr);
                let mut shape = Vec::with_capacity(elems.len());
                for &elem_bits in elems.iter() {
                    let elem_obj = obj_from_bits(elem_bits);
                    let Some(val) = to_i64(elem_obj).or_else(|| {
                        bigint_ptr_from_bits(elem_bits).and_then(|ptr| bigint_ref(ptr).to_i64())
                    }) else {
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
                    shape.push(val as isize);
                }
                shape
            } else {
                let itemsize = fmt.itemsize as i128;
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
            let product = match memoryview_shape_product(&shape) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            if product.saturating_mul(fmt.itemsize as i128) != nbytes {
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
                stride = stride.saturating_mul(shape[idx].max(1));
            }
            let out_ptr = alloc_memoryview_shaped(
                _py,
                memoryview_owner_bits(view_ptr),
                memoryview_offset(view_ptr),
                fmt.itemsize,
                memoryview_readonly(view_ptr),
                format_bits,
                shape,
                strides,
            );
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_memoryview_tobytes(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(bits);
        let ptr = match obj.as_ptr() {
            Some(ptr) => ptr,
            None => return raise_exception::<_>(_py, "TypeError", "tobytes expects a memoryview"),
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_MEMORYVIEW {
                return raise_exception::<_>(_py, "TypeError", "tobytes expects a memoryview");
            }
            if memoryview_released(ptr) {
                return raise_released_memoryview(_py);
            }
            let out = match memoryview_collect_bytes(ptr) {
                Some(val) => val,
                None => return MoltObject::none().bits(),
            };
            let out_ptr = alloc_bytes(_py, &out);
            if out_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

unsafe fn memoryview_tolist_recursive(
    _py: &PyToken<'_>,
    data: &[u8],
    fmt: MemoryViewFormat,
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
            let item_offset = base_offset.checked_add((i as isize).saturating_mul(strides[dim]))?;
            let scalar = unsafe { memoryview_read_scalar(_py, data, item_offset, fmt) }?;
            items.push(scalar);
        }
    } else {
        for i in 0..dim_len {
            let child_offset =
                base_offset.checked_add((i as isize).saturating_mul(strides[dim]))?;
            let child = unsafe {
                memoryview_tolist_recursive(_py, data, fmt, shape, strides, dim + 1, child_offset)
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
            let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                Some(fmt) => fmt,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "memoryview: unsupported format for tolist()",
                    );
                }
            };
            let owner_bits = memoryview_owner_bits(ptr);
            let owner = obj_from_bits(owner_bits);
            let owner_ptr = match owner.as_ptr() {
                Some(ptr) => ptr,
                None => return MoltObject::none().bits(),
            };
            let data = match bytes_like_slice_raw(owner_ptr) {
                Some(slice) => slice,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "memoryview: tolist() requires a bytes-like exporter",
                    );
                }
            };
            let shape = memoryview_shape(ptr).unwrap_or(&[]);
            let strides = memoryview_strides(ptr).unwrap_or(&[]);
            if shape.is_empty() || memoryview_ndim(ptr) == 0 {
                let scalar = match memoryview_read_scalar(_py, data, memoryview_offset(ptr), fmt) {
                    Some(bits) => bits,
                    None => return MoltObject::none().bits(),
                };
                return scalar;
            }
            match memoryview_tolist_recursive(
                _py,
                data,
                fmt,
                shape,
                strides,
                0,
                memoryview_offset(ptr),
            ) {
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
            if memoryview_released(ptr) {
                return raise_released_memoryview(_py);
            }
            let ndim = memoryview_ndim(ptr);
            if ndim == 0 {
                return raise_exception::<_>(_py, "TypeError", "invalid indexing of 0-dim memory");
            }
            if ndim > 1 {
                return raise_exception::<_>(
                    _py,
                    "NotImplementedError",
                    "multi-dimensional sub-views are not implemented",
                );
            }
            let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                Some(fmt) => fmt,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "memoryview: unsupported format for count()",
                    );
                }
            };
            let owner_bits = memoryview_owner_bits(ptr);
            let owner = obj_from_bits(owner_bits);
            let Some(owner_ptr) = owner.as_ptr() else {
                return MoltObject::none().bits();
            };
            let Some(base) = bytes_like_slice_raw(owner_ptr) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "memoryview: count() requires a bytes-like exporter",
                );
            };
            let len = memoryview_len(ptr);
            let offset = memoryview_offset(ptr);
            let stride = memoryview_stride(ptr);
            let mut count = 0i64;
            for idx in 0..len {
                let item_offset = offset.saturating_add((idx as isize).saturating_mul(stride));
                let Some(item_bits) = memoryview_read_scalar(_py, base, item_offset, fmt) else {
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
                return raise_exception::<_>(
                    _py,
                    "NotImplementedError",
                    "multi-dimensional lookup is not implemented",
                );
            }
            let fmt = match memoryview_format_from_bits(memoryview_format_bits(ptr)) {
                Some(fmt) => fmt,
                None => {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        "memoryview: unsupported format for index()",
                    );
                }
            };
            let owner_bits = memoryview_owner_bits(ptr);
            let owner = obj_from_bits(owner_bits);
            let Some(owner_ptr) = owner.as_ptr() else {
                return MoltObject::none().bits();
            };
            let Some(base) = bytes_like_slice_raw(owner_ptr) else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "memoryview: index() requires a bytes-like exporter",
                );
            };
            let len = memoryview_len(ptr);
            let offset = memoryview_offset(ptr);
            let stride = memoryview_stride(ptr);
            for idx in 0..len {
                let item_offset = offset.saturating_add((idx as isize).saturating_mul(stride));
                let Some(item_bits) = memoryview_read_scalar(_py, base, item_offset, fmt) else {
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
            memoryview_mark_released(ptr);
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

#[repr(C)]
#[derive(Clone, Copy)]
pub struct BufferExport {
    pub ptr: *mut u8,
    pub len: u64,
    pub readonly: u32,
    pub ndim: u32,
    pub itemsize: u64,
    pub offset: isize,
    pub owner: u64,
    pub base: u64,
    pub shape: [isize; MOLT_BUFFER_MAX_NDIM],
    pub strides: [isize; MOLT_BUFFER_MAX_NDIM],
    pub format: [u8; MOLT_BUFFER_FORMAT_CAP],
}

impl Default for BufferExport {
    fn default() -> Self {
        let mut format = [0; MOLT_BUFFER_FORMAT_CAP];
        format[0] = b'B';
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
            readonly: 1,
            ndim: 1,
            itemsize: 1,
            offset: 0,
            owner: 0,
            base: 0,
            shape: [0; MOLT_BUFFER_MAX_NDIM],
            strides: [0; MOLT_BUFFER_MAX_NDIM],
            format,
        }
    }
}

impl BufferExport {
    fn from_typed_storage(storage: &TypedStridedStorage) -> Option<Self> {
        if storage.shape.len() != storage.strides.len()
            || storage.shape.len() > MOLT_BUFFER_MAX_NDIM
        {
            return None;
        }
        let mut out = Self {
            ptr: storage.data,
            len: u64::try_from(storage.len).ok()?,
            readonly: if storage.readonly { 1 } else { 0 },
            ndim: storage.shape.len() as u32,
            itemsize: u64::try_from(storage.itemsize).ok()?,
            offset: storage.offset,
            base: storage.base_bits,
            format: storage.format,
            ..Self::default()
        };
        for (slot, value) in out.shape.iter_mut().zip(storage.shape.iter().copied()) {
            *slot = value;
        }
        for (slot, value) in out.strides.iter_mut().zip(storage.strides.iter().copied()) {
            *slot = value;
        }
        Some(out)
    }
}

/// # Safety
/// Caller must ensure `out_ptr` is valid and writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_buffer_export(obj_bits: u64, out_ptr: *mut BufferExport) -> i32 {
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
                Err(_) => return 1,
            };
            let Some(export) = BufferExport::from_typed_storage(&storage) else {
                return 1;
            };
            *out_ptr = export;
            0
        })
    }
}
