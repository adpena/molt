//! Bytes and bytearray operations — extracted from ops.rs for tree-shaking.
//!
//! Each `pub extern "C" fn molt_bytes_*` / `molt_bytearray_*` is a separate
//! linker symbol so that `wasm-ld --gc-sections` can drop unused entries.

use crate::object::buffer_exports::bytearray_mutate;
use crate::object::ops_encoding::DecodeFailure;
use crate::*;
use molt_obj_model::MoltObject;
use num_traits::{Signed, ToPrimitive};
use std::sync::OnceLock;

mod sequence_methods;

pub use sequence_methods::*;

#[path = "ops_bytes_ascii.rs"]
mod ops_bytes_ascii;
pub(crate) use ops_bytes_ascii::bytes_ascii_space;
pub use ops_bytes_ascii::{
    molt_bytearray_capitalize, molt_bytearray_center, molt_bytearray_expandtabs,
    molt_bytearray_isalnum, molt_bytearray_isalpha, molt_bytearray_isascii, molt_bytearray_isdigit,
    molt_bytearray_islower, molt_bytearray_isspace, molt_bytearray_istitle, molt_bytearray_isupper,
    molt_bytearray_ljust, molt_bytearray_lower, molt_bytearray_removeprefix,
    molt_bytearray_removesuffix, molt_bytearray_rjust, molt_bytearray_swapcase,
    molt_bytearray_title, molt_bytearray_upper, molt_bytearray_zfill, molt_bytes_capitalize,
    molt_bytes_center, molt_bytes_expandtabs, molt_bytes_isalnum, molt_bytes_isalpha,
    molt_bytes_isascii, molt_bytes_isdigit, molt_bytes_islower, molt_bytes_isspace,
    molt_bytes_istitle, molt_bytes_isupper, molt_bytes_ljust, molt_bytes_lower,
    molt_bytes_removeprefix, molt_bytes_removesuffix, molt_bytes_rjust, molt_bytes_swapcase,
    molt_bytes_title, molt_bytes_upper, molt_bytes_zfill,
};

use super::ops::parse_codec_arg;

/// Both byte-storage families use the same index-or-simple-buffer protocol.
/// Coercion and exporter callbacks finish before observing the haystack address.
pub(crate) fn bytes_contains_builtin(py: &PyToken<'_>, container: u64, needle: u64) -> u64 {
    use molt_cpython_abi::{
        abi_types::PyBUF_SIMPLE,
        api::{buffer::PyObject_GetBuffer, memory::MemoryViewLease, refcount::OwnedPyObject},
        bridge::GLOBAL_BRIDGE,
    };
    let has_index = crate::builtins::numbers::index_integral_payload_bits(needle).is_some()
        || unsafe { crate::builtins::attr::has_special_method(py, needle, b"__index__") };
    if exception_pending(py) {
        return MoltObject::none().bits();
    }
    let number = if has_index {
        crate::builtins::numbers::index_ssize_clamped_from_obj(
            py,
            needle,
            "object cannot be interpreted as an integer",
        )
    } else {
        None
    };
    if let Some(number) = number {
        if !(0..=255).contains(&number) {
            return raise_exception(py, "ValueError", "byte must be in range(0, 256)");
        }
        let ptr = obj_from_bits(container).as_ptr().unwrap();
        let hay = unsafe { std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr)) };
        return MoltObject::from_bool(memchr::memchr(number as u8, hay).is_some()).bits();
    }
    // CPython _Py_bytes_contains clears ANY failed index conversion before
    // requesting PyBUF_SIMPLE, including a raising user __index__ callback.
    if exception_pending(py) {
        clear_exception(py);
    }
    if !crate::object::buffer_exports::supports_buffer(py, needle) {
        return raise_exception(
            py,
            "TypeError",
            &format!(
                "a bytes-like object is required, not '{}'",
                type_name(py, obj_from_bits(needle)),
            ),
        );
    }
    unsafe {
        // Ordinary byte payloads need no lease: there are no remaining callbacks
        // before the search. Observe both addresses only after index coercion.
        if let Some(ptr) = obj_from_bits(needle).as_ptr()
            && matches!(object_type_id(ptr), TYPE_ID_BYTES | TYPE_ID_BYTEARRAY)
        {
            let needle = std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr));
            let hay_ptr = obj_from_bits(container).as_ptr().unwrap();
            let hay = std::slice::from_raw_parts(bytes_data(hay_ptr), bytes_len(hay_ptr));
            return MoltObject::from_bool(bytes_find_impl(hay, needle) >= 0).bits();
        }
        let object = OwnedPyObject::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(needle));
        if object.as_ptr().is_null() {
            crate::cpython_abi_hooks::propagate_native_failure(py, "membership buffer projection");
            return MoltObject::none().bits();
        }
        let lease =
            match MemoryViewLease::acquire(object.as_ptr(), PyBUF_SIMPLE, PyObject_GetBuffer) {
                Ok(lease) => lease,
                Err(molt_cpython_abi::ErrorIndicatorSet) => {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        py,
                        "membership buffer acquisition",
                    );
                    return MoltObject::none().bits();
                }
            };
        let view = &*lease.descriptor();
        if view.len < 0 || (view.len != 0 && view.buf.is_null()) {
            return raise_exception(py, "BufferError", "invalid membership buffer span");
        }
        let needle = if view.len == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(view.buf.cast::<u8>(), view.len as usize)
        };
        let ptr = obj_from_bits(container).as_ptr().unwrap();
        let hay = std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr));
        MoltObject::from_bool(bytes_find_impl(hay, needle) >= 0).bits()
    }
}

fn bytes_like_arg_or_type_error<F>(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    make_type_error: F,
) -> Result<&'static [u8], u64>
where
    F: FnOnce() -> String,
{
    match unsafe { bytes_like_slice_checked(ptr) } {
        Ok(slice) => Ok(slice),
        Err(BytesLikeSliceError::ReleasedMemoryView) => Err(raise_released_memoryview::<u64>(_py)),
        Err(BytesLikeSliceError::NonContiguousMemoryView) => Err(raise_exception::<u64>(
            _py,
            "BufferError",
            "memoryview: underlying buffer is not C-contiguous",
        )),
        Err(BytesLikeSliceError::NotBytesLike) => {
            let msg = make_type_error();
            Err(raise_exception::<u64>(_py, "TypeError", &msg))
        }
    }
}

fn bytes_join_part_or_type_error<F>(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    make_type_error: F,
) -> Result<&'static [u8], u64>
where
    F: FnOnce() -> String,
{
    match unsafe { bytes_like_slice_checked(ptr) } {
        Ok(slice) => Ok(slice),
        Err(
            BytesLikeSliceError::ReleasedMemoryView
            | BytesLikeSliceError::NonContiguousMemoryView
            | BytesLikeSliceError::NotBytesLike,
        ) => {
            let msg = make_type_error();
            Err(raise_exception::<u64>(_py, "TypeError", &msg))
        }
    }
}

pub(super) fn collect_bytearray_assign_bytes(_py: &PyToken<'_>, bits: u64) -> Option<Vec<u8>> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                return Some(bytes_like_slice_raw(ptr).unwrap_or(&[]).to_vec());
            }
            if type_id == TYPE_ID_STRING {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "can assign only bytes, buffers, or iterables of ints in range(0, 256)",
                );
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                if memoryview_released(ptr) {
                    let _ = raise_released_memoryview::<u64>(_py);
                    return None;
                }
                if let Some(slice) = memoryview_bytes_slice(ptr) {
                    return Some(slice.to_vec());
                }
                return memoryview_collect_bytes(ptr);
            }
        }
    }
    let mut iter = crate::object::iterable::OwnedIterator::new(_py, bits)?;
    bytes_collect_from_iter(_py, &mut iter, BytesCtorKind::Bytearray, 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_extend(bytearray_bits: u64, other_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.extend expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray.extend expects bytearray",
                );
            }
        }
        let Some(payload) = collect_bytearray_assign_bytes(_py, other_bits) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let Some(required_len) = bytearray_len(bytearray_ptr).checked_add(payload.len()) else {
                return raise_exception::<_>(_py, "MemoryError", "bytearray allocation failed");
            };
            if bytearray_mutate(_py, bytearray_ptr, required_len, |vec| {
                vec.extend_from_slice(&payload)
            })
            .is_none()
            {
                return MoltObject::none().bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_append(bytearray_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.append expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray.append expects bytearray",
                );
            }
        }
        let Some(byte) = bytes_item_to_u8(_py, val_bits, BytesCtorKind::Bytearray) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let Some(required_len) = bytearray_len(bytearray_ptr).checked_add(1) else {
                return raise_exception::<_>(_py, "MemoryError", "bytearray allocation failed");
            };
            if bytearray_mutate(_py, bytearray_ptr, required_len, |vec| vec.push(byte)).is_none() {
                return MoltObject::none().bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_fill_range(
    bytearray_bits: u64,
    start_bits: u64,
    stop_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "bytearray_fill_range expects bytearray",
            );
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray_fill_range expects bytearray",
                );
            }
        }
        let start = index_i64_from_obj(
            _py,
            start_bits,
            "bytearray fill range indices must be integers",
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let stop = index_i64_from_obj(
            _py,
            stop_bits,
            "bytearray fill range indices must be integers",
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(byte) = bytes_item_to_u8(_py, val_bits, BytesCtorKind::Bytearray) else {
            return MoltObject::none().bits();
        };
        unsafe {
            let elems = bytearray_vec(bytearray_ptr);
            let len = elems.len() as i64;
            if start < 0 || stop < start || stop > len {
                return raise_exception::<_>(
                    _py,
                    "IndexError",
                    "bytearray fill range out of range",
                );
            }
            elems[start as usize..stop as usize].fill(byte);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_clear(bytearray_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.clear expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(_py, "TypeError", "bytearray.clear expects bytearray");
            }
            let _ = bytearray_mutate(_py, bytearray_ptr, 0, Vec::clear);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_copy(bytearray_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.copy expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(_py, "TypeError", "bytearray.copy expects bytearray");
            }
            let data = bytes_like_slice(bytearray_ptr).unwrap_or(&[]);
            let ptr = alloc_bytearray(_py, data);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_insert(
    bytearray_bits: u64,
    index_bits: u64,
    val_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.insert expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray.insert expects bytearray",
                );
            }
            let mut idx = index_i64_from_obj(
                _py,
                index_bits,
                "bytearray indices must be integers or have an __index__ method",
            );
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let Some(byte) = bytes_item_to_u8(_py, val_bits, BytesCtorKind::Bytearray) else {
                return MoltObject::none().bits();
            };
            let len = bytearray_len(bytearray_ptr) as i64;
            if idx < 0 {
                idx += len;
            }
            if idx < 0 {
                idx = 0;
            }
            if idx > len {
                idx = len;
            }
            let Some(required_len) = bytearray_len(bytearray_ptr).checked_add(1) else {
                return raise_exception::<_>(_py, "MemoryError", "bytearray allocation failed");
            };
            if bytearray_mutate(_py, bytearray_ptr, required_len, |vec| {
                vec.insert(idx as usize, byte)
            })
            .is_none()
            {
                return MoltObject::none().bits();
            }
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_pop(bytearray_bits: u64, index_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let index_obj = obj_from_bits(index_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.pop expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(_py, "TypeError", "bytearray.pop expects bytearray");
            }
            let mut idx = if index_obj.is_none() {
                -1
            } else {
                index_i64_from_obj(
                    _py,
                    index_bits,
                    "bytearray indices must be integers or have an __index__ method",
                )
            };
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            let len = bytearray_len(bytearray_ptr) as i64;
            if len == 0 {
                return raise_exception::<_>(_py, "IndexError", "pop from empty bytearray");
            }
            if idx < 0 {
                idx += len;
            }
            if idx < 0 || idx >= len {
                return raise_exception::<_>(_py, "IndexError", "pop index out of range");
            }
            let Some(out) = bytearray_mutate(_py, bytearray_ptr, len as usize - 1, |elems| {
                elems.remove(idx as usize)
            }) else {
                return MoltObject::none().bits();
            };
            MoltObject::from_int(i64::from(out)).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_remove(bytearray_bits: u64, val_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.remove expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray.remove expects bytearray",
                );
            }
            let Some(byte) = bytes_item_to_u8(_py, val_bits, BytesCtorKind::Bytearray) else {
                return MoltObject::none().bits();
            };
            let elems = bytearray_vec_ref(bytearray_ptr);
            if let Some(pos) = elems.iter().position(|item| *item == byte) {
                let len = elems.len();
                let _ = bytearray_mutate(_py, bytearray_ptr, len - 1, |elems| elems.remove(pos));
                return MoltObject::none().bits();
            }
            raise_exception::<_>(_py, "ValueError", "value not found in bytearray")
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_reverse(bytearray_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bytearray_obj = obj_from_bits(bytearray_bits);
        let Some(bytearray_ptr) = bytearray_obj.as_ptr() else {
            return raise_exception::<_>(_py, "TypeError", "bytearray.reverse expects bytearray");
        };
        unsafe {
            if object_type_id(bytearray_ptr) != TYPE_ID_BYTEARRAY {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "bytearray.reverse expects bytearray",
                );
            }
            bytearray_vec(bytearray_ptr).reverse();
            MoltObject::none().bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_resize(bytearray_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let size = index_i64_from_obj(
            _py,
            size_bits,
            "bytearray.resize() argument must be integer",
        );
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if size < 0 {
            let msg = format!("Can only resize to positive sizes, got {size}");
            return raise_exception::<_>(_py, "ValueError", &msg);
        }
        let Ok(size) = usize::try_from(size) else {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "cannot fit 'int' into an index-sized integer",
            );
        };
        crate::object::buffer_exports::bytearray_resize(_py, bytearray_bits, size);
        MoltObject::none().bits()
    })
}

fn bytes_decode_impl(
    _py: &PyToken<'_>,
    hay_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    type_id: u32,
) -> u64 {
    let hay = obj_from_bits(hay_bits);
    let Some(hay_ptr) = hay.as_ptr() else {
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(hay_ptr) != type_id {
            return MoltObject::none().bits();
        }
        let encoding = match parse_codec_arg(_py, encoding_bits, "decode", "encoding", "utf-8") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let errors = match parse_codec_arg(_py, errors_bits, "decode", "errors", "strict") {
            Some(val) => val,
            None => return MoltObject::none().bits(),
        };
        let bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);

        match decode_bytes_text(&encoding, &errors, bytes) {
            Ok((text_bytes, _label)) => {
                let ptr = alloc_string(_py, &text_bytes);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                MoltObject::from_ptr(ptr).bits()
            }
            Err(DecodeTextError::UnknownEncoding(name)) => {
                let msg = format!("unknown encoding: {name}");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
            Err(DecodeTextError::UnknownErrorHandler(name)) => {
                let msg = format!("unknown error handler name '{name}'");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
            Err(DecodeTextError::Failure(DecodeFailure::Byte { pos, message, .. }, label)) => {
                raise_unicode_decode_error(_py, &label, hay_bits, pos, pos + 1, message)
            }
            Err(DecodeTextError::Failure(
                DecodeFailure::Range {
                    start,
                    end,
                    message,
                },
                label,
            )) => raise_unicode_decode_error(
                _py,
                &label,
                hay_bits,
                start,
                end.saturating_add(1),
                message,
            ),
            Err(DecodeTextError::Failure(DecodeFailure::UnknownErrorHandler(name), _label)) => {
                let msg = format!("unknown error handler name '{name}'");
                raise_exception::<_>(_py, "LookupError", &msg)
            }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_decode(hay_bits: u64, encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_decode_impl(_py, hay_bits, encoding_bits, errors_bits, TYPE_ID_BYTES)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_decode(
    hay_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_decode_impl(_py, hay_bits, encoding_bits, errors_bits, TYPE_ID_BYTEARRAY)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_replace(
    hay_bits: u64,
    needle_bits: u64,
    replacement_bits: u64,
    count_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let replacement = obj_from_bits(replacement_bits);
        let count_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(count_bits))
        );
        let count = index_i64_from_obj(_py, count_bits, &count_err);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                let repl_ptr = match replacement.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let repl_bytes = match bytes_like_arg_or_type_error(_py, repl_ptr, || {
                    format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, replacement)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                let out = if count < 0 {
                    match replace_bytes_impl(hay_bytes, needle_bytes, repl_bytes) {
                        Some(out) => out,
                        None => return MoltObject::none().bits(),
                    }
                } else {
                    replace_bytes_impl_limit(hay_bytes, needle_bytes, repl_bytes, count as usize)
                };
                let ptr = alloc_bytes(_py, &out);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        }
        MoltObject::none().bits()
    })
}

fn bytes_hex_sep_from_bits(_py: &PyToken<'_>, sep_bits: u64) -> Result<Option<String>, u64> {
    if sep_bits == 0 || obj_from_bits(sep_bits).is_none() {
        return Ok(None);
    }
    let sep_obj = obj_from_bits(sep_bits);
    let Some(sep_ptr) = sep_obj.as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "sep must be str or bytes",
        ));
    };
    unsafe {
        let type_id = object_type_id(sep_ptr);
        if type_id == TYPE_ID_STRING {
            let bytes = std::slice::from_raw_parts(string_bytes(sep_ptr), string_len(sep_ptr));
            let Ok(sep_str) = std::str::from_utf8(bytes) else {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "sep must be str or bytes",
                ));
            };
            if sep_str.chars().count() != 1 {
                return Err(raise_exception::<_>(
                    _py,
                    "ValueError",
                    "sep must be length 1",
                ));
            }
            return Ok(Some(sep_str.to_string()));
        }
        if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
            let bytes = bytes_like_slice(sep_ptr).unwrap_or(&[]);
            if bytes.len() != 1 {
                return Err(raise_exception::<_>(
                    _py,
                    "ValueError",
                    "sep must be length 1",
                ));
            }
            let ch = char::from(bytes[0]);
            return Ok(Some(ch.to_string()));
        }
    }
    Err(raise_exception::<_>(
        _py,
        "TypeError",
        "sep must be str or bytes",
    ))
}

fn bytes_hex_string(bytes: &[u8], sep: Option<&str>, bytes_per_sep: i64) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    if bytes.is_empty() {
        return String::new();
    }
    let hex_len = bytes.len() * 2;
    let Some(sep_str) = sep else {
        // SIMD fast path for no-separator hex encoding
        let mut raw: Vec<u8> = Vec::with_capacity(hex_len);
        let mut i = 0usize;
        #[cfg(target_arch = "aarch64")]
        {
            if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
                unsafe {
                    use std::arch::aarch64::*;
                    let hex_lut = vld1q_u8(b"0123456789abcdef".as_ptr());
                    let mask_lo = vdupq_n_u8(0x0F);
                    while i + 16 <= bytes.len() {
                        let chunk = vld1q_u8(bytes.as_ptr().add(i));
                        let hi_nibbles = vshrq_n_u8(chunk, 4);
                        let lo_nibbles = vandq_u8(chunk, mask_lo);
                        let hi_hex = vqtbl1q_u8(hex_lut, hi_nibbles);
                        let lo_hex = vqtbl1q_u8(hex_lut, lo_nibbles);
                        let zipped_lo = vzip1q_u8(hi_hex, lo_hex);
                        let zipped_hi = vzip2q_u8(hi_hex, lo_hex);
                        let len = raw.len();
                        raw.set_len(len + 32);
                        vst1q_u8(raw.as_mut_ptr().add(len), zipped_lo);
                        vst1q_u8(raw.as_mut_ptr().add(len + 16), zipped_hi);
                        i += 16;
                    }
                }
            }
        }
        #[cfg(target_arch = "x86_64")]
        {
            if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("ssse3") {
                unsafe {
                    use std::arch::x86_64::*;
                    let mask_lo = _mm_set1_epi8(0x0F);
                    let hex_lut = _mm_setr_epi8(
                        b'0' as i8, b'1' as i8, b'2' as i8, b'3' as i8, b'4' as i8, b'5' as i8,
                        b'6' as i8, b'7' as i8, b'8' as i8, b'9' as i8, b'a' as i8, b'b' as i8,
                        b'c' as i8, b'd' as i8, b'e' as i8, b'f' as i8,
                    );
                    while i + 16 <= bytes.len() {
                        let chunk = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
                        let hi_nibbles = _mm_and_si128(_mm_srli_epi16(chunk, 4), mask_lo);
                        let lo_nibbles = _mm_and_si128(chunk, mask_lo);
                        let hi_hex = _mm_shuffle_epi8(hex_lut, hi_nibbles);
                        let lo_hex = _mm_shuffle_epi8(hex_lut, lo_nibbles);
                        let interleaved_lo = _mm_unpacklo_epi8(hi_hex, lo_hex);
                        let interleaved_hi = _mm_unpackhi_epi8(hi_hex, lo_hex);
                        let len = raw.len();
                        raw.set_len(len + 32);
                        _mm_storeu_si128(raw.as_mut_ptr().add(len) as *mut __m128i, interleaved_lo);
                        _mm_storeu_si128(
                            raw.as_mut_ptr().add(len + 16) as *mut __m128i,
                            interleaved_hi,
                        );
                        i += 16;
                    }
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            if cfg!(target_feature = "simd128") && bytes.len() >= 16 {
                unsafe {
                    use std::arch::wasm32::*;
                    let mask_lo = u8x16_splat(0x0F);
                    let hex_lut = v128_load(b"0123456789abcdef".as_ptr() as *const v128);
                    while i + 16 <= bytes.len() {
                        let chunk = v128_load(bytes.as_ptr().add(i) as *const v128);
                        let hi_nibbles = v128_and(u16x8_shr(chunk, 4), mask_lo);
                        let lo_nibbles = v128_and(chunk, mask_lo);
                        let hi_hex = i8x16_swizzle(hex_lut, hi_nibbles);
                        let lo_hex = i8x16_swizzle(hex_lut, lo_nibbles);
                        // Interleave hi and lo hex chars
                        let interleaved_lo = i8x16_shuffle::<
                            0,
                            16,
                            1,
                            17,
                            2,
                            18,
                            3,
                            19,
                            4,
                            20,
                            5,
                            21,
                            6,
                            22,
                            7,
                            23,
                        >(hi_hex, lo_hex);
                        let interleaved_hi = i8x16_shuffle::<
                            8,
                            24,
                            9,
                            25,
                            10,
                            26,
                            11,
                            27,
                            12,
                            28,
                            13,
                            29,
                            14,
                            30,
                            15,
                            31,
                        >(hi_hex, lo_hex);
                        let len = raw.len();
                        raw.set_len(len + 32);
                        v128_store(raw.as_mut_ptr().add(len) as *mut v128, interleaved_lo);
                        v128_store(raw.as_mut_ptr().add(len + 16) as *mut v128, interleaved_hi);
                        i += 16;
                    }
                }
            }
        }
        // Scalar tail
        for &b in &bytes[i..] {
            raw.push(HEX[(b >> 4) as usize]);
            raw.push(HEX[(b & 0xF) as usize]);
        }
        // SAFETY: all bytes are valid ASCII hex characters
        return unsafe { String::from_utf8_unchecked(raw) };
    };
    let group = bytes_per_sep.unsigned_abs() as usize;
    let separators = bytes
        .len()
        .saturating_sub(1)
        .checked_div(group)
        .unwrap_or(0);
    let mut out = String::with_capacity(hex_len + separators * sep_str.len());
    if bytes_per_sep > 0 {
        for (idx, &b) in bytes.iter().enumerate() {
            if idx > 0 && idx % group == 0 {
                out.push_str(sep_str);
            }
            out.push(char::from(HEX[(b >> 4) as usize]));
            out.push(char::from(HEX[(b & 0xF) as usize]));
        }
    } else {
        let mut first_group = bytes.len() % group;
        if first_group == 0 {
            first_group = group;
        }
        for (idx, &b) in bytes.iter().enumerate() {
            if idx == first_group
                || (idx > first_group && (idx - first_group).is_multiple_of(group))
            {
                out.push_str(sep_str);
            }
            out.push(char::from(HEX[(b >> 4) as usize]));
            out.push(char::from(HEX[(b & 0xF) as usize]));
        }
    }
    out
}

pub(crate) fn bytes_hex_from_bits(
    _py: &PyToken<'_>,
    bytes: &[u8],
    sep_bits: u64,
    bytes_per_sep_bits: u64,
) -> u64 {
    // CPython converts bytes_per_sep (the second positional arg) during argument
    // parsing, BEFORE the separator is validated, so a non-int bytes_per_sep
    // raises "'X' object cannot be interpreted as an integer" first — e.g.
    // b'abcd'.hex(123, 'x') reports the 'x' bytes_per_sep error, not the 123 sep
    // error. (Verified against CPython 3.12/3.13/3.14.)
    let bytes_per_sep = if bytes_per_sep_bits == missing_bits(_py) {
        1
    } else {
        let bps_msg = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(bytes_per_sep_bits))
        );
        index_i64_from_obj(_py, bytes_per_sep_bits, &bps_msg)
    };
    if exception_pending(_py) {
        return MoltObject::none().bits();
    }
    let sep_opt = if sep_bits == missing_bits(_py) {
        None
    } else {
        match bytes_hex_sep_from_bits(_py, sep_bits) {
            Ok(sep) => sep,
            Err(err_bits) => return err_bits,
        }
    };
    // bytes_per_sep == 0 means "no grouping" in CPython: the (already validated)
    // separator is unused and the plain ungrouped hex string is returned. Forcing
    // sep to None routes bytes_hex_string through its no-separator fast path, which
    // never consults bytes_per_sep, so a zero group can never divide by zero.
    let sep_for_grouping = if bytes_per_sep == 0 { None } else { sep_opt };
    let text = bytes_hex_string(bytes, sep_for_grouping.as_deref(), bytes_per_sep);
    let ptr = alloc_string(_py, text.as_bytes());
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(ptr).bits()
}

fn bytes_translate_impl(
    _py: &PyToken<'_>,
    hay_bytes: &[u8],
    table_bits: u64,
    delete_bits: u64,
) -> Result<Vec<u8>, u64> {
    let table_obj = obj_from_bits(table_bits);
    let table_opt = if table_obj.is_none() {
        None
    } else {
        let table_ptr = match table_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, table_obj)
                );
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            }
        };
        let table_bytes = bytes_like_arg_or_type_error(_py, table_ptr, || {
            format!(
                "a bytes-like object is required, not '{}'",
                type_name(_py, table_obj)
            )
        })?;
        if table_bytes.len() != 256 {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                "translation table must be 256 characters long",
            ));
        }
        Some(table_bytes)
    };
    let delete_bytes = if is_missing_bits(_py, delete_bits) {
        &[]
    } else {
        let delete_obj = obj_from_bits(delete_bits);
        let delete_ptr = match delete_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, delete_obj)
                );
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            }
        };
        bytes_like_arg_or_type_error(_py, delete_ptr, || {
            format!(
                "a bytes-like object is required, not '{}'",
                type_name(_py, delete_obj)
            )
        })?
    };
    if hay_bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut delete_map = [false; 256];
    for &b in delete_bytes {
        delete_map[b as usize] = true;
    }
    let mut out = Vec::with_capacity(hay_bytes.len());
    match table_opt {
        Some(table) => {
            for &b in hay_bytes {
                if delete_map[b as usize] {
                    continue;
                }
                out.push(table[b as usize]);
            }
        }
        None => {
            for &b in hay_bytes {
                if delete_map[b as usize] {
                    continue;
                }
                out.push(b);
            }
        }
    }
    Ok(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_translate(hay_bits: u64, table_bits: u64, delete_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let out = match bytes_translate_impl(_py, hay_bytes, table_bits, delete_bits) {
                Ok(out) => out,
                Err(err_bits) => return err_bits,
            };
            let ptr = alloc_bytes(_py, &out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_translate(
    hay_bits: u64,
    table_bits: u64,
    delete_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            let out = match bytes_translate_impl(_py, hay_bytes, table_bits, delete_bits) {
                Ok(out) => out,
                Err(err_bits) => return err_bits,
            };
            let ptr = alloc_bytearray(_py, &out);
            if ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_maketrans(from_bits: u64, to_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let from_obj = obj_from_bits(from_bits);
        let to_obj = obj_from_bits(to_bits);
        let from_ptr = match from_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, from_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        let to_ptr = match to_obj.as_ptr() {
            Some(ptr) => ptr,
            None => {
                let msg = format!(
                    "a bytes-like object is required, not '{}'",
                    type_name(_py, to_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        };
        let from_bytes = match bytes_like_arg_or_type_error(_py, from_ptr, || {
            format!(
                "a bytes-like object is required, not '{}'",
                type_name(_py, from_obj)
            )
        }) {
            Ok(slice) => slice,
            Err(bits) => return bits,
        };
        let to_bytes = match bytes_like_arg_or_type_error(_py, to_ptr, || {
            format!(
                "a bytes-like object is required, not '{}'",
                type_name(_py, to_obj)
            )
        }) {
            Ok(slice) => slice,
            Err(bits) => return bits,
        };
        if from_bytes.len() != to_bytes.len() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "maketrans arguments must have same length",
            );
        }
        let mut table = [0u8; 256];
        for (idx, slot) in table.iter_mut().enumerate() {
            *slot = idx as u8;
        }
        for (from_byte, to_byte) in from_bytes.iter().zip(to_bytes.iter()) {
            table[*from_byte as usize] = *to_byte;
        }
        let ptr = alloc_bytes(_py, &table);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

fn fromhex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn bytes_fromhex_parse(_py: &PyToken<'_>, text: &[u8]) -> Result<Vec<u8>, u64> {
    // CPython permits ASCII whitespace only *between* byte pairs, never between the
    // two nibbles of a single byte. The accepted set is Py_ISSPACE on ASCII:
    // space, \t, \n, \r, \v (0x0b), \f (0x0c). Rust's is_ascii_whitespace() omits
    // 0x0b, so match the byte explicitly for full parity.
    fn is_fromhex_space(b: u8) -> bool {
        matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
    }
    let mut out: Vec<u8> = Vec::new();
    let mut idx = 0usize;
    while idx < text.len() {
        // Skip whitespace between byte pairs.
        while idx < text.len() && is_fromhex_space(text[idx]) {
            idx += 1;
        }
        if idx >= text.len() {
            break;
        }
        let Some(hi) = fromhex_nibble(text[idx]) else {
            let msg = format!("non-hexadecimal number found in fromhex() arg at position {idx}");
            return Err(raise_exception::<_>(_py, "ValueError", &msg));
        };
        idx += 1;
        // The low nibble must immediately follow the high nibble; whitespace is
        // not permitted inside a byte pair.
        if idx >= text.len() {
            // The high nibble was the final character (no trailing whitespace).
            // CPython 3.14 reports an even-length error here; 3.12/3.13 report the
            // position immediately after the high nibble.
            let msg = if crate::object::ops_sys::runtime_target_at_least(_py, 3, 14) {
                "fromhex() arg must contain an even number of hexadecimal digits".to_string()
            } else {
                format!("non-hexadecimal number found in fromhex() arg at position {idx}")
            };
            return Err(raise_exception::<_>(_py, "ValueError", &msg));
        }
        let Some(lo) = fromhex_nibble(text[idx]) else {
            let msg = format!("non-hexadecimal number found in fromhex() arg at position {idx}");
            return Err(raise_exception::<_>(_py, "ValueError", &msg));
        };
        idx += 1;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_fromhex(cls_bits: u64, text_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let text_obj = obj_from_bits(text_bits);
        let Some(text_ptr) = text_obj.as_ptr() else {
            let msg = format!(
                "fromhex() argument must be str, not {}",
                type_name(_py, text_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(text_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "fromhex() argument must be str, not {}",
                    type_name(_py, text_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let text = std::slice::from_raw_parts(string_bytes(text_ptr), string_len(text_ptr));
            let out = match bytes_fromhex_parse(_py, text) {
                Ok(out) => out,
                Err(err_bits) => return err_bits,
            };
            let bytes_ptr = alloc_bytes(_py, &out);
            if bytes_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
            let builtins = builtin_classes(_py);
            if cls_bits == builtins.bytes {
                return bytes_bits;
            }
            if !issubclass_bits(cls_bits, builtins.bytes) {
                dec_ref_bits(_py, bytes_bits);
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "fromhex() requires a bytes subclass",
                );
            }
            let res_bits = call_callable1(_py, cls_bits, bytes_bits);
            dec_ref_bits(_py, bytes_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            res_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_fromhex(cls_bits: u64, text_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let text_obj = obj_from_bits(text_bits);
        let Some(text_ptr) = text_obj.as_ptr() else {
            let msg = format!(
                "fromhex() argument must be str, not {}",
                type_name(_py, text_obj)
            );
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        unsafe {
            if object_type_id(text_ptr) != TYPE_ID_STRING {
                let msg = format!(
                    "fromhex() argument must be str, not {}",
                    type_name(_py, text_obj)
                );
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
            let text = std::slice::from_raw_parts(string_bytes(text_ptr), string_len(text_ptr));
            let out = match bytes_fromhex_parse(_py, text) {
                Ok(out) => out,
                Err(err_bits) => return err_bits,
            };
            let ba_ptr = alloc_bytearray(_py, &out);
            if ba_ptr.is_null() {
                return MoltObject::none().bits();
            }
            let ba_bits = MoltObject::from_ptr(ba_ptr).bits();
            let builtins = builtin_classes(_py);
            if cls_bits == builtins.bytearray {
                return ba_bits;
            }
            if !issubclass_bits(cls_bits, builtins.bytearray) {
                dec_ref_bits(_py, ba_bits);
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "fromhex() requires a bytearray subclass",
                );
            }
            let res_bits = call_callable1(_py, cls_bits, ba_bits);
            dec_ref_bits(_py, ba_bits);
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            res_bits
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_hex(hay_bits: u64, sep_bits: u64, bytes_per_sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTES {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            bytes_hex_from_bits(_py, hay_bytes, sep_bits, bytes_per_sep_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_hex(hay_bits: u64, sep_bits: u64, bytes_per_sep_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let Some(hay_ptr) = hay.as_ptr() else {
            return MoltObject::none().bits();
        };
        unsafe {
            if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                return MoltObject::none().bits();
            }
            let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
            bytes_hex_from_bits(_py, hay_bytes, sep_bits, bytes_per_sep_bits)
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_replace(
    hay_bits: u64,
    needle_bits: u64,
    replacement_bits: u64,
    count_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let hay = obj_from_bits(hay_bits);
        let needle = obj_from_bits(needle_bits);
        let replacement = obj_from_bits(replacement_bits);
        let count_err = format!(
            "'{}' object cannot be interpreted as an integer",
            type_name(_py, obj_from_bits(count_bits))
        );
        let count = index_i64_from_obj(_py, count_bits, &count_err);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Some(hay_ptr) = hay.as_ptr() {
            unsafe {
                if object_type_id(hay_ptr) != TYPE_ID_BYTEARRAY {
                    return MoltObject::none().bits();
                }
                let hay_bytes = bytes_like_slice(hay_ptr).unwrap_or(&[]);
                let needle_ptr = match needle.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, needle)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let needle_bytes = match bytes_like_arg_or_type_error(_py, needle_ptr, || {
                    format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, needle)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                let repl_ptr = match replacement.as_ptr() {
                    Some(ptr) => ptr,
                    None => {
                        let msg = format!(
                            "a bytes-like object is required, not '{}'",
                            type_name(_py, replacement)
                        );
                        return raise_exception::<_>(_py, "TypeError", &msg);
                    }
                };
                let repl_bytes = match bytes_like_arg_or_type_error(_py, repl_ptr, || {
                    format!(
                        "a bytes-like object is required, not '{}'",
                        type_name(_py, replacement)
                    )
                }) {
                    Ok(slice) => slice,
                    Err(bits) => return bits,
                };
                let out = if count < 0 {
                    match replace_bytes_impl(hay_bytes, needle_bytes, repl_bytes) {
                        Some(out) => out,
                        None => return MoltObject::none().bits(),
                    }
                } else {
                    replace_bytes_impl_limit(hay_bytes, needle_bytes, repl_bytes, count as usize)
                };
                let ptr = alloc_bytearray(_py, &out);
                if ptr.is_null() {
                    return MoltObject::none().bits();
                }
                return MoltObject::from_ptr(ptr).bits();
            }
        }
        MoltObject::none().bits()
    })
}

#[derive(Clone, Copy)]
pub(super) enum BytesCtorKind {
    Bytes,
    Bytearray,
}

impl BytesCtorKind {
    pub(crate) fn name(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes",
            BytesCtorKind::Bytearray => "bytearray",
        }
    }

    fn ctor_label(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes()",
            BytesCtorKind::Bytearray => "bytearray()",
        }
    }

    fn range_error(self) -> &'static str {
        match self {
            BytesCtorKind::Bytes => "bytes must be in range(0, 256)",
            BytesCtorKind::Bytearray => "byte must be in range(0, 256)",
        }
    }

    fn non_iterable_message(self, type_name: &str) -> String {
        format!("cannot convert '{}' object to {}", type_name, self.name())
    }

    fn arg_type_message(self, arg: &str, type_name: &str) -> String {
        format!(
            "{} argument '{}' must be str, not {}",
            self.ctor_label(),
            arg,
            type_name
        )
    }
}

fn bytes_from_count(_py: &PyToken<'_>, len: usize, kind: BytesCtorKind) -> u64 {
    if matches!(kind, BytesCtorKind::Bytearray) {
        let ptr = alloc_bytearray_with_len(_py, len);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        return MoltObject::from_ptr(ptr).bits();
    }
    let ptr = alloc_inline_bytes_with_len(_py, len, InlineBytesKind::Bytes);
    if ptr.is_null() {
        return MoltObject::none().bits();
    }
    unsafe {
        let data_ptr = super::layout::InlineBytesStorage::data(ptr);
        std::ptr::write_bytes(data_ptr, 0, len);
    }
    MoltObject::from_ptr(ptr).bits()
}

pub(super) fn bytes_item_to_u8(_py: &PyToken<'_>, bits: u64, kind: BytesCtorKind) -> Option<u8> {
    let type_name = class_name_for_error(type_of_bits(_py, bits));
    let msg = format!("'{}' object cannot be interpreted as an integer", type_name);
    let val = crate::builtins::numbers::index_bigint_from_obj(_py, bits, &msg)?;
    val.to_u8()
        .or_else(|| raise_exception::<_>(_py, "ValueError", kind.range_error()))
}

fn bytes_collect_from_iter(
    _py: &PyToken<'_>,
    iter: &mut crate::object::iterable::OwnedIterator<'_, '_>,
    kind: BytesCtorKind,
    capacity: usize,
) -> Option<Vec<u8>> {
    collect_byte_items(_py, kind, capacity, || {
        let item = iter.next().ok()?;
        let Some(item) = item else {
            return Some(None);
        };
        let byte = bytes_item_to_u8(_py, item, kind);
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(_py, item));
        byte.map(Some)
    })
}

fn collect_byte_items(
    _py: &PyToken<'_>,
    _kind: BytesCtorKind,
    capacity: usize,
    mut next: impl FnMut() -> Option<Option<u8>>,
) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    if out.try_reserve(capacity).is_err() {
        return raise_exception::<_>(_py, "MemoryError", "bytes allocation failed");
    }
    while let Some(byte) = next()? {
        if out.try_reserve(1).is_err() {
            return raise_exception::<_>(_py, "MemoryError", "bytes allocation failed");
        }
        out.push(byte);
    }
    Some(out)
}

/// Native iterator custody adapts the existing linked slot protocol to the
/// same byte collector as managed iteration. No copied sequence algorithm.
fn native_bytes_iterable(py: &PyToken<'_>, bits: u64, kind: BytesCtorKind) -> Option<Vec<u8>> {
    use molt_cpython_abi::api::{abstract_number, errors, object, refcount::OwnedPyObject};
    let native =
        unsafe { crate::object::foreign::foreign_ptr_from_obj(obj_from_bits(bits).as_ptr()?) };
    let source = std::ptr::with_exposed_provenance_mut(native);
    let iterator = unsafe { object::PyObject_GetIter(source) };
    if iterator.is_null() {
        if unsafe {
            errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast(),
            )
        } != 0
        {
            unsafe { errors::PyErr_Clear() };
            let name = unsafe {
                molt_cpython_abi::api::typeobj::object_type_name_with_precision(source, 200)
            };
            return raise_exception(py, "TypeError", &kind.non_iterable_message(&name));
        }
        crate::cpython_abi_hooks::propagate_native_failure(py, "bytes native iterator acquisition");
        return None;
    }
    let iterator = unsafe { OwnedPyObject::from_owned(iterator) };
    let capacity = if matches!(kind, BytesCtorKind::Bytes) {
        let hint = unsafe { object::PyObject_LengthHint(source, 64) };
        if hint < 0 {
            crate::cpython_abi_hooks::propagate_native_failure(py, "bytes native length hint");
            return None;
        }
        hint as usize
    } else {
        0
    };
    collect_byte_items(py, kind, capacity, || {
        let item = unsafe { object::PyIter_Next(iterator.as_ptr()) };
        if item.is_null() {
            if unsafe { errors::PyErr_Occurred() }.is_null() {
                return Some(None);
            }
            crate::cpython_abi_hooks::propagate_native_failure(py, "bytes native iteration");
            return None;
        }
        let item = unsafe { OwnedPyObject::from_owned(item) };
        let value =
            unsafe { abstract_number::PyNumber_AsSsize_t(item.as_ptr(), std::ptr::null_mut()) };
        if value == -1 && !unsafe { errors::PyErr_Occurred() }.is_null() {
            crate::cpython_abi_hooks::propagate_native_failure(
                py,
                "bytes native integer conversion",
            );
            return None;
        }
        match u8::try_from(value) {
            Ok(byte) => Some(Some(byte)),
            Err(_) => raise_exception(py, "ValueError", kind.range_error()),
        }
    })
}

fn bytes_constructor_iter<'a, 'py>(
    py: &'a PyToken<'py>,
    source: u64,
    kind: BytesCtorKind,
) -> Option<crate::object::iterable::OwnedIterator<'a, 'py>> {
    let iter = crate::object::iterable::OwnedIterator::new(py, source);
    if iter.is_none() && exception_pending(py) {
        let error = molt_exception_last();
        let replace =
            crate::builtins::exceptions::exception_matches_builtin_name(py, error, "TypeError");
        if replace {
            clear_exception(py);
        }
        dec_ref_bits(py, error);
        if replace {
            raise_exception::<()>(
                py,
                "TypeError",
                &kind.non_iterable_message(&type_name(py, obj_from_bits(source))),
            );
        }
    }
    iter
}

/// __bytes__ is a physical descriptor, distinct from the converting builtin.
/// Inherited calls on a subtype return a fresh exact value; an override may
/// legally return any bytes subtype, and bytes(obj) preserves that result.
pub(crate) extern "C" fn bytes_bytes(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = obj_from_bits(bits)
            .as_ptr()
            .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_BYTES })
        else {
            return raise_exception::<_>(
                py,
                "TypeError",
                "descriptor '__bytes__' requires a 'bytes' object",
            );
        };
        if type_of_bits(py, bits) == builtin_classes(py).bytes {
            inc_ref_bits(py, bits);
            return bits;
        }
        let out = unsafe { alloc_bytes(py, bytes_like_slice_raw(ptr).unwrap()) };
        if out.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(out).bits()
        }
    })
}

/// Byte constructors share the integral protocol, including the documented
/// TypeError fallback from an index provider to the buffer/iterable path.
fn byte_count(py: &PyToken<'_>, bits: u64) -> Result<Option<usize>, ()> {
    let integral = crate::builtins::numbers::index_bigint_integral_bits(bits).is_some();
    let index =
        integral || unsafe { crate::builtins::attr::has_special_method(py, bits, b"__index__") };
    if exception_pending(py) {
        return Err(());
    }
    if !index {
        return Ok(None);
    }
    let value = crate::builtins::numbers::index_bigint_from_obj(
        py,
        bits,
        "object cannot be interpreted as an integer",
    );
    let Some(value) = value else {
        let error = molt_exception_last();
        let fallback =
            crate::builtins::exceptions::exception_matches_builtin_name(py, error, "TypeError");
        if fallback {
            clear_exception(py);
        }
        dec_ref_bits(py, error);
        return if fallback { Ok(None) } else { Err(()) };
    };
    if value.is_negative() {
        raise_exception::<()>(py, "ValueError", "negative count");
        return Err(());
    }
    let Some(count) = value
        .to_usize()
        .filter(|&count| count <= isize::MAX as usize)
    else {
        raise_exception::<()>(
            py,
            "OverflowError",
            "cannot fit 'int' into an index-sized integer",
        );
        return Err(());
    };
    Ok(Some(count))
}

/// Reuse the admitted typed/strided projection. Native exports stay owned
/// through the copy; managed memoryviews retain their existing caller lease.
pub(in crate::object) fn byte_buffer(
    py: &PyToken<'_>,
    source: u64,
) -> Option<(Vec<u8>, Option<PtrDropGuard>)> {
    if let Some(ptr) = obj_from_bits(source).as_ptr()
        && unsafe { object_type_id(ptr) == crate::TYPE_ID_FOREIGN }
    {
        let object = std::ptr::with_exposed_provenance_mut(unsafe {
            crate::object::foreign::foreign_ptr_from_obj(ptr)
        });
        return match unsafe {
            molt_cpython_abi::api::buffer::with_buffer_descriptor(object, |view| {
                crate::object::memoryview::collect_bytes_from_descriptor(py, view)
            })
        } {
            Ok(Some(bytes)) => Some((bytes, None)),
            Ok(None) => None, // The runtime collector set its own error.
            Err(molt_cpython_abi::ErrorIndicatorSet) => {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "native byte buffer acquisition",
                );
                None
            }
        };
    }
    let view = molt_memoryview_new(source);
    if exception_pending(py) {
        dec_ref_bits(py, view);
        return None;
    }
    let ptr = obj_from_bits(view).as_ptr()?;
    let owner = PtrDropGuard::new(ptr);
    let bytes = unsafe { memoryview_collect_bytes(ptr) };
    let Some(bytes) = bytes else {
        if !exception_pending(py) {
            raise_exception::<()>(py, "BufferError", "invalid constructor buffer geometry");
        }
        return None;
    };
    Some((bytes, Some(owner)))
}

/// C object conversion shares the constructor's special/buffer/iterable owner,
/// but PyObject_Bytes never interprets an index-only value as a byte count.
pub(crate) fn bytes_from_object(py: &PyToken<'_>, bits: u64, special: bool) -> u64 {
    bytes_from_obj_impl(py, bits, BytesCtorKind::Bytes, false, special)
}

fn bytes_from_obj_impl(
    py: &PyToken<'_>,
    bits: u64,
    kind: BytesCtorKind,
    allow_count: bool,
    special: bool,
) -> u64 {
    if special && matches!(kind, BytesCtorKind::Bytes) {
        let method =
            unsafe { crate::builtins::attr::lookup_special_method(py, bits, b"__bytes__") };
        if let Some(method) = method {
            let result = unsafe { call_callable0(py, method) };
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, method));
            if exception_pending(py) {
                molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, result));
                return MoltObject::none().bits();
            }
            let valid = obj_from_bits(result).as_ptr().is_some_and(|ptr| unsafe {
                object_type_id(ptr) == TYPE_ID_BYTES
                    || (object_type_id(ptr) == crate::TYPE_ID_FOREIGN
                        && molt_cpython_abi::api::strings::PyBytes_Check(
                            std::ptr::with_exposed_provenance_mut(
                                crate::object::foreign::foreign_ptr_from_obj(ptr),
                            ),
                        ) != 0)
            });
            if valid {
                return result;
            }
            let name = if let Some(pointer) = obj_from_bits(result).as_ptr()
                && unsafe { object_type_id(pointer) == crate::TYPE_ID_FOREIGN }
            {
                unsafe {
                    molt_cpython_abi::api::typeobj::object_type_name_with_precision(
                        std::ptr::with_exposed_provenance_mut(
                            crate::object::foreign::foreign_ptr_from_obj(pointer),
                        ),
                        200,
                    )
                }
            } else {
                let name = type_name(py, obj_from_bits(result)).into_owned();
                String::from_utf8_lossy(&name.as_bytes()[..name.len().min(200)]).into_owned()
            };
            let failure = raise_exception::<u64>(
                py,
                "TypeError",
                &format!("__bytes__ returned non-bytes (type {name})"),
            );
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, result));
            return failure;
        }
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
    }
    if obj_from_bits(bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
    {
        if allow_count {
            return raise_exception::<_>(py, "TypeError", "string argument without an encoding");
        }
        let name = type_name(py, obj_from_bits(bits)).into_owned();
        let name = String::from_utf8_lossy(&name.as_bytes()[..name.len().min(200)]);
        return raise_exception::<_>(
            py,
            "TypeError",
            &format!("cannot convert '{name}' object to bytes"),
        );
    }
    if allow_count {
        match byte_count(py, bits) {
            Ok(Some(count)) => return bytes_from_count(py, count, kind),
            Ok(None) => {}
            Err(()) => return MoltObject::none().bits(),
        }
    }
    let bytes = if crate::object::buffer_exports::supports_buffer(py, bits) {
        let Some((bytes, _export)) = byte_buffer(py, bits) else {
            return MoltObject::none().bits();
        };
        bytes
    } else if obj_from_bits(bits)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == crate::TYPE_ID_FOREIGN })
    {
        let Some(bytes) = native_bytes_iterable(py, bits, kind) else {
            return MoltObject::none().bits();
        };
        bytes
    } else {
        let Some(mut iter) = bytes_constructor_iter(py, bits, kind) else {
            return MoltObject::none().bits();
        };
        let capacity = if matches!(kind, BytesCtorKind::Bytes) {
            let Some(hint) = crate::object::iterable::length_hint(py, bits) else {
                return MoltObject::none().bits();
            };
            hint
        } else {
            0
        };
        let Some(bytes) = bytes_collect_from_iter(py, &mut iter, kind, capacity) else {
            return MoltObject::none().bits();
        };
        bytes
    };
    let ptr = match kind {
        BytesCtorKind::Bytes => alloc_bytes(py, &bytes),
        BytesCtorKind::Bytearray => alloc_bytearray(py, &bytes),
    };
    if ptr.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// bytearray.__init__ is an in-place transaction with CPython's partial mutation
/// semantics: argument parsing precedes clearing; conversion/iteration follows
/// clearing; each callback ends before reloading current length and storage.
pub(crate) fn bytearray_init_from_arguments(
    py: &PyToken<'_>,
    receiver: u64,
    bound: [Option<u64>; 3],
) -> u64 {
    let Some(ptr) = obj_from_bits(receiver)
        .as_ptr()
        .filter(|&ptr| unsafe { object_type_id(ptr) == TYPE_ID_BYTEARRAY })
    else {
        return raise_exception::<_>(
            py,
            "TypeError",
            "descriptor '__init__' requires a 'bytearray' object",
        );
    };
    if unsafe { bytearray_mutate(py, ptr, 0, Vec::clear) }.is_none() {
        return MoltObject::none().bits();
    }
    let Some(source) = bound[0] else {
        if bound[1].is_some() || bound[2].is_some() {
            return raise_exception::<_>(
                py,
                "TypeError",
                if bound[1].is_some() {
                    "encoding without a string argument"
                } else {
                    "errors without a string argument"
                },
            );
        }
        return MoltObject::none().bits();
    };
    let string = obj_from_bits(source)
        .as_ptr()
        .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING });
    if string {
        let Some(encoding) = bound[1] else {
            return raise_exception::<_>(py, "TypeError", "string argument without an encoding");
        };
        let encoded = molt_bytes_from_str(
            source,
            encoding,
            bound[2].unwrap_or_else(|| MoltObject::none().bits()),
        );
        if exception_pending(py) {
            dec_ref_bits(py, encoded);
            return MoltObject::none().bits();
        }
        let _result = molt_bytearray_extend(receiver, encoded);
        dec_ref_bits(py, encoded);
        return MoltObject::none().bits();
    }
    if bound[1].is_some() || bound[2].is_some() {
        return raise_exception::<_>(
            py,
            "TypeError",
            if bound[1].is_some() {
                "encoding without a string argument"
            } else {
                "errors without a string argument"
            },
        );
    }
    match byte_count(py, source) {
        Ok(Some(count)) => {
            if count != 0 {
                unsafe {
                    bytearray_mutate(py, ptr, count, |bytes| {
                        bytes.resize(count, 0);
                        bytes.fill(0);
                    });
                }
            }
            return MoltObject::none().bits();
        }
        Ok(None) => {}
        Err(()) => return MoltObject::none().bits(),
    }
    if crate::object::buffer_exports::supports_buffer(py, source) {
        let Some((bytes, _export)) = byte_buffer(py, source) else {
            return MoltObject::none().bits();
        };
        unsafe {
            bytearray_mutate(py, ptr, bytes.len(), |target| {
                target.clear();
                target.extend_from_slice(&bytes);
            });
        }
        return MoltObject::none().bits();
    }
    if let Some(sequence) = obj_from_bits(source).as_ptr()
        && unsafe {
            matches!(object_type_id(sequence), TYPE_ID_LIST | TYPE_ID_TUPLE)
                && crate::object::iterable::builtin_receiver(py, sequence)
        }
    {
        let Some(items) = (unsafe {
            crate::object::seq_access::snapshot(
                py,
                sequence,
                "bytearray constructor snapshot failed",
            )
        }) else {
            return MoltObject::none().bits();
        };
        if unsafe { bytearray_mutate(py, ptr, items.len(), |bytes| bytes.resize(items.len(), 0)) }
            .is_none()
        {
            return MoltObject::none().bits();
        }
        let mut complete = true;
        for (index, &item) in items.iter().enumerate() {
            if type_of_bits(py, item) != builtin_classes(py).int {
                complete = false;
                break;
            }
            let Some(value) = bytes_item_to_u8(py, item, BytesCtorKind::Bytearray) else {
                return MoltObject::none().bits();
            };
            unsafe {
                bytearray_mutate(py, ptr, items.len(), |bytes| bytes[index] = value);
            }
        }
        if complete {
            return MoltObject::none().bits();
        }
        // The optimization owns no source snapshot across Python callbacks.
        drop(items);
        if unsafe { bytearray_mutate(py, ptr, 0, Vec::clear) }.is_none() {
            return MoltObject::none().bits();
        }
    }
    let Some(mut iter) = bytes_constructor_iter(py, source, BytesCtorKind::Bytearray) else {
        return MoltObject::none().bits();
    };
    while let Ok(Some(item)) = iter.next() {
        let value = bytes_item_to_u8(py, item, BytesCtorKind::Bytearray);
        dec_ref_bits(py, item);
        let Some(value) = value else {
            break;
        };
        let Some(len) = (unsafe { bytearray_len(ptr) }).checked_add(1) else {
            raise_exception::<()>(py, "MemoryError", "bytearray allocation failed");
            break;
        };
        if unsafe { bytearray_mutate(py, ptr, len, |bytes| bytes.push(value)) }.is_none() {
            break;
        }
    }
    MoltObject::none().bits()
}

fn bytes_from_str_impl(
    _py: &PyToken<'_>,
    src_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
    kind: BytesCtorKind,
) -> u64 {
    let encoding_obj = obj_from_bits(encoding_bits);
    let errors_obj = obj_from_bits(errors_bits);
    let encoding = if encoding_obj.is_none() {
        None
    } else {
        let Some(encoding) = string_obj_to_owned(encoding_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, encoding_bits));
            let msg = kind.arg_type_message("encoding", &type_name);
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        Some(encoding)
    };
    let errors = if errors_obj.is_none() {
        None
    } else {
        let Some(errors) = string_obj_to_owned(errors_obj) else {
            let type_name = class_name_for_error(type_of_bits(_py, errors_bits));
            let msg = kind.arg_type_message("errors", &type_name);
            return raise_exception::<_>(_py, "TypeError", &msg);
        };
        Some(errors)
    };
    let src_obj = obj_from_bits(src_bits);
    let Some(src_ptr) = src_obj.as_ptr() else {
        if encoding.is_some() {
            return raise_exception::<_>(_py, "TypeError", "encoding without a string argument");
        }
        if errors.is_some() {
            return raise_exception::<_>(_py, "TypeError", "errors without a string argument");
        }
        return MoltObject::none().bits();
    };
    unsafe {
        if object_type_id(src_ptr) != TYPE_ID_STRING {
            if encoding.is_some() {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "encoding without a string argument",
                );
            }
            if errors.is_some() {
                return raise_exception::<_>(_py, "TypeError", "errors without a string argument");
            }
            return MoltObject::none().bits();
        }
    }
    let Some(encoding) = encoding else {
        return raise_exception::<_>(_py, "TypeError", "string argument without an encoding");
    };
    let bytes = unsafe { std::slice::from_raw_parts(string_bytes(src_ptr), string_len(src_ptr)) };
    let out = match encode_string_with_errors(bytes, &encoding, errors.as_deref()) {
        Ok(bytes) => bytes,
        Err(EncodeError::UnknownEncoding(name)) => {
            let msg = format!("unknown encoding: {name}");
            return raise_exception::<_>(_py, "LookupError", &msg);
        }
        Err(EncodeError::UnknownErrorHandler(name)) => {
            let msg = format!("unknown error handler name '{name}'");
            return raise_exception::<_>(_py, "LookupError", &msg);
        }
        Err(EncodeError::InvalidChar {
            encoding,
            code,
            pos,
            limit,
        }) => {
            let reason = encode_error_reason(encoding, code, limit);
            return raise_unicode_encode_error::<_>(_py, encoding, src_bits, pos, pos + 1, &reason);
        }
    };
    let out_ptr = match kind {
        BytesCtorKind::Bytes => alloc_bytes(_py, &out),
        BytesCtorKind::Bytearray => alloc_bytearray(_py, &out),
    };
    if out_ptr.is_null() {
        return MoltObject::none().bits();
    }
    MoltObject::from_ptr(out_ptr).bits()
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_from_obj(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_from_obj_impl(_py, bits, BytesCtorKind::Bytes, true, true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_from_obj(bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_from_obj_impl(_py, bits, BytesCtorKind::Bytearray, true, true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytes_from_str(src_bits: u64, encoding_bits: u64, errors_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_from_str_impl(
            _py,
            src_bits,
            encoding_bits,
            errors_bits,
            BytesCtorKind::Bytes,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_bytearray_from_str(
    src_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        bytes_from_str_impl(
            _py,
            src_bits,
            encoding_bits,
            errors_bits,
            BytesCtorKind::Bytearray,
        )
    })
}
