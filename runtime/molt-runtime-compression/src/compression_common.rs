use crate::bridge::*;
use molt_cpython_abi as abi;
use molt_runtime_capi::{NativeObjectOwner, retire_native_reference};
use molt_runtime_core::prelude::*;

const COMPRESSION_STREAMS_BUFFER_SIZE: i64 = 8192;
#[unsafe(no_mangle)]
pub extern "C" fn molt_compression_streams_buffer_size() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        int_bits_from_i64(_py, COMPRESSION_STREAMS_BUFFER_SIZE)
    })
}

/// CPython 3.12 Clinic's explicit contiguous-buffer check. Later supported
/// versions rely on the SIMPLE provider contract instead. Geometry belongs to
/// the existing ABI descriptor; this helper owns only the argument diagnostic.
pub(crate) fn validate_clinic_buffer(
    py: &molt_runtime_core::PyToken,
    input: &molt_runtime_capi::ByteInput<'_>,
    bits: u64,
    function: &str,
    argument: &str,
) -> bool {
    if molt_runtime_core::rt_target_at_least(3, 13) || input.is_c_contiguous() {
        return true;
    }
    let name = type_name(py, obj_from_bits(bits));
    let message = format!("{function}() {argument} must be contiguous buffer, not {name}");
    raise_exception(py, BuiltinExc::TypeError, &message);
    false
}

pub(crate) enum CodecIndexRole {
    CInt,
    OutputSize,
}

// Argument Clinic uses the existing C-API integer authority for these roles.
// Integer callbacks happen before the stream mutex is acquired.
pub(crate) fn codec_index(py: &PyToken, bits: u64, role: CodecIndexRole) -> Result<i64, u64> {
    if let Some(value) = obj_from_bits(bits).as_int() {
        let fits = match role {
            CodecIndexRole::CInt => i32::try_from(value).is_ok(),
            CodecIndexRole::OutputSize => isize::try_from(value).is_ok(),
        };
        if fits {
            return Ok(value);
        }
    }
    unsafe {
        let object = NativeObjectOwner::from_owned(retire_native_reference(|| {
            abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits)
        }));
        if object.as_ptr().is_null() {
            return Err(MoltObject::none().bits());
        }
        let value = if matches!(role, CodecIndexRole::CInt) {
            retire_native_reference(|| abi::api::numbers::PyLong_AsInt(object.as_ptr())) as i64
        } else {
            let integer = NativeObjectOwner::from_owned(retire_native_reference(|| {
                abi::api::abstract_number::PyNumber_Index(object.as_ptr())
            }));
            if integer.as_ptr().is_null() {
                return Err(MoltObject::none().bits());
            }
            retire_native_reference(|| abi::api::numbers::PyLong_AsSsize_t(integer.as_ptr())) as i64
        };
        if exception_pending(py) {
            Err(MoltObject::none().bits())
        } else {
            Ok(value)
        }
    }
}

/// The target-versioned unsigned converter used by CPython's LZMA memlimit.
/// Only the native LZMA module consumes it; WASM uses `lzma_wasm`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn codec_uint64(py: &PyToken, bits: u64) -> Result<u64, u64> {
    if let Some(value) = obj_from_bits(bits).as_int().filter(|value| *value >= 0) {
        return Ok(value as u64);
    }
    unsafe {
        let object = NativeObjectOwner::from_owned(retire_native_reference(|| {
            abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(bits)
        }));
        if object.as_ptr().is_null() {
            return Err(MoltObject::none().bits());
        }
        let value = if molt_runtime_core::rt_target_at_least(3, 14) {
            let mut value = 0u64;
            // Native endian | unsigned buffer | reject negative | allow index.
            let size = retire_native_reference(|| {
                abi::api::numbers::PyLong_AsNativeBytes(
                    object.as_ptr(),
                    (&mut value as *mut u64).cast(),
                    8,
                    3 | 4 | 8 | 16,
                )
            });
            if size > 8 {
                return Err(raise_exception(
                    py,
                    BuiltinExc::OverflowError,
                    "Python int too large for C uint64_t",
                ));
            }
            value
        } else {
            retire_native_reference(|| {
                abi::api::numbers::PyLong_AsUnsignedLongLong(object.as_ptr())
            })
        };
        if exception_pending(py) {
            Err(MoltObject::none().bits())
        } else {
            Ok(value)
        }
    }
}
