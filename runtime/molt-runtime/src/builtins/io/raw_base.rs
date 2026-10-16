//! RawIOBase's native defaults over the public, virtual raw-stream protocol.
//! Pinned CPython Modules/_io/iobase.c owns the call/error order. No physical
//! backend is required, and no Rust storage borrow spans a Python callback.
use super::buffered::{call, owned, trap_eintr};
use super::*;
use crate::object::buffer_exports::{bytearray_require_unexported, bytearray_resize};
use molt_runtime_core::OwnedRuntimeValue;
use num_traits::ToPrimitive;

fn bytes(py: &PyToken<'_>, value: &[u8]) -> u64 {
    let pointer = alloc_bytes(py, value);
    if pointer.is_null() {
        MoltObject::none().bits()
    } else {
        MoltObject::from_ptr(pointer).bits()
    }
}

/// PyNumber_AsSsize_t(result, ValueError), without replacing an exception raised
/// by the user's __index__. Immediate integers retain the existing scalar path.
pub(super) fn count(py: &PyToken<'_>, result: u64) -> Option<i64> {
    let converted = if let Some(value) = crate::builtins::numbers::index_i64_integral_bits(result) {
        isize::try_from(value).ok()
    } else {
        let message = format!(
            "'{}' object cannot be interpreted as an integer",
            crate::type_name(py, obj_from_bits(result))
        );
        crate::builtins::numbers::index_bigint_from_obj(py, result, &message)?.to_isize()
    };
    match converted {
        Some(value) => Some(value as i64),
        None => {
            raise_exception::<u64>(
                py,
                "ValueError",
                &format!(
                    "cannot fit '{}' into an index-sized integer",
                    crate::type_name(py, obj_from_bits(result))
                ),
            );
            None
        }
    }
}

pub(crate) extern "C" fn raw_io_base_read(self_bits: u64, size_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _self = OwnedRuntimeValue::retain(py.core_token(), self_bits);
        let size = crate::builtins::numbers::index_ssize_from_obj(py, size_bits);
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        if size < 0 {
            return call(py, self_bits, b"readall", None);
        }
        let pointer = crate::object::builders::alloc_bytearray_with_len(py, size as usize);
        if pointer.is_null() {
            return MoltObject::none().bits();
        }
        let buffer = owned(py, MoltObject::from_ptr(pointer).bits());
        let result = owned(py, call(py, self_bits, b"readinto", Some(buffer.bits())));
        if exception_pending(py) || obj_from_bits(result.bits()).is_none() {
            return result.into_bits();
        }
        let converted = count(py, result.bits());
        // CPython retires the result immediately after __index__: its last
        // owner may mutate the retained bytearray before validation/copy.
        drop(result);
        let Some(count) = converted else {
            return MoltObject::none().bits();
        };
        let checked_count = crate::object::ops_sys::runtime_target_at_least(py, 3, 13);
        if count < 0 {
            return if checked_count {
                raise_exception(
                    py,
                    "ValueError",
                    &format!("readinto returned {count} outside buffer size {size}"),
                )
            } else {
                raise_exception(
                    py,
                    "SystemError",
                    "Negative size passed to PyBytes_FromStringAndSize",
                )
            };
        }
        let count = count as usize;
        let take = crate::object::ops_sys::runtime_target_at_least(py, 3, 15);
        // 3.12's hostile over-count copy is not memory-safe. Reject it rather
        // than reading beyond the initialized bytearray, including callback shrink.
        let available = if take {
            size as usize
        } else {
            unsafe { bytearray_len(pointer) }
        };
        if count > available || (checked_count && count > size as usize) {
            return raise_exception(
                py,
                "ValueError",
                &format!("readinto returned {count} outside buffer size {size}"),
            );
        }
        if take {
            // 3.15 resizes then takes the bytes, leaving a retained callback
            // bytearray empty. Both changes enter the existing counted owner.
            if !bytearray_resize(py, buffer.bits(), count) {
                return MoltObject::none().bits();
            }
            if let Err(error) = unsafe { bytearray_require_unexported(py, pointer) } {
                return error;
            }
        }
        let result = unsafe { bytes(py, &bytearray_vec_ref(pointer)[..count]) };
        if obj_from_bits(result).is_none() {
            return result;
        }
        if take && !bytearray_resize(py, buffer.bits(), 0) {
            dec_ref_bits(py, result);
            return MoltObject::none().bits();
        }
        result
    })
}

fn append(py: &PyToken<'_>, output: &mut Vec<u8>, value: &[u8]) -> Result<(), u64> {
    if output
        .len()
        .checked_add(value.len())
        .is_none_or(|length| length > isize::MAX as usize)
        || output.try_reserve(value.len()).is_err()
    {
        return Err(raise_exception(
            py,
            "MemoryError",
            "raw readall result is too large",
        ));
    }
    output.extend_from_slice(value);
    Ok(())
}

/// The pre-3.15 join keeps every Python chunk alive until completion and
/// retires them in list order (last to first), including on callback failure.
struct ReadChunks<'py>(Vec<OwnedRuntimeValue<'py>>);
impl Drop for ReadChunks<'_> {
    fn drop(&mut self) {
        while self.0.pop().is_some() {}
    }
}

pub(crate) extern "C" fn raw_io_base_readall(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let _self = OwnedRuntimeValue::retain(py.core_token(), self_bits);
        let modern = crate::object::ops_sys::runtime_target_at_least(py, 3, 15);
        let size = MoltObject::from_int(default_buffer_size(py)).bits();
        let mut chunks = ReadChunks(Vec::new());
        let mut output = Vec::new();
        loop {
            let result = owned(py, call(py, self_bits, b"read", Some(size)));
            if exception_pending(py) {
                if trap_eintr(py) {
                    continue;
                }
                return MoltObject::none().bits();
            }
            if obj_from_bits(result.bits()).is_none() {
                if chunks.0.is_empty() && output.is_empty() {
                    return result.into_bits();
                }
                break;
            }
            let Some(pointer) = obj_from_bits(result.bits())
                .as_ptr()
                .filter(|&pointer| unsafe { object_type_id(pointer) == TYPE_ID_BYTES })
            else {
                return raise_exception(py, "TypeError", "read() should return bytes");
            };
            let value =
                unsafe { std::slice::from_raw_parts(bytes_data(pointer), bytes_len(pointer)) };
            if value.is_empty() {
                break;
            }
            if modern {
                if let Err(error) = append(py, &mut output, value) {
                    return error;
                }
            } else {
                if chunks.0.try_reserve(1).is_err() {
                    return raise_exception(py, "MemoryError", "cannot retain raw readall chunks");
                }
                chunks.0.push(result);
            }
        }
        if chunks.0.len() == 1 {
            let pointer = obj_from_bits(chunks.0[0].bits()).as_ptr().unwrap();
            let class = unsafe { object_class_bits(pointer) };
            if class == 0 || class == builtin_classes(py).bytes {
                // Legacy bytes.join returns its sole exact bytes chunk unchanged.
                return chunks.0.pop().unwrap().into_bits();
            }
        }
        for chunk in &chunks.0 {
            let pointer = obj_from_bits(chunk.bits()).as_ptr().unwrap();
            let value =
                unsafe { std::slice::from_raw_parts(bytes_data(pointer), bytes_len(pointer)) };
            if let Err(error) = append(py, &mut output, value) {
                return error;
            }
        }
        bytes(py, &output)
    })
}

/// CPython publishes this abstract default as METH_VARARGS: positional arity is
/// intentionally unrestricted, while keywords are refused before the body.
pub(crate) extern "C" fn raw_io_base_readinto(args: u64, kwargs: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(call) =
            crate::builtins::native_arguments::NativeArguments::read(py, "readinto", args, kwargs)
        else {
            return MoltObject::none().bits();
        };
        if !call.keyword_view().is_empty() {
            return raise_exception(
                py,
                "TypeError",
                "_RawIOBase.readinto() takes no keyword arguments",
            );
        }
        raise_exception(py, "NotImplementedError", "")
    })
}
