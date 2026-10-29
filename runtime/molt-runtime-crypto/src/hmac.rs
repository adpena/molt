use crate::bridge::*;
use crate::hashlib::{HashHandle, build_hash_handle, builtin_hmac_name, clinic_input, hash_input};
use abi::api::{errors, mapping, object, strings};
use abi::bridge::GLOBAL_BRIDGE;
use molt_cpython_abi as abi;
use molt_runtime_capi::{ByteInput, NativeObjectOwner, retire_native_reference};
use molt_runtime_core::prelude::*;

#[derive(Clone)]
pub struct HmacHandle {
    inner: HashHandle,
    outer: HashHandle,
    #[allow(dead_code)]
    digest_size: usize,
    #[allow(dead_code)]
    block_size: usize,
}

impl HmacHandle {
    fn finalize_bytes(&self) -> Result<Vec<u8>, crate::hashlib::HashError> {
        let inner = self.inner.finalize_bytes(None)?;
        let mut outer = self.outer.clone();
        outer.update(&inner);
        outer.finalize_bytes(None)
    }
}

// Closed roles, also declared at the intrinsic's source boundary. The buffer
// order is observably different: singleshot Clinic acquires both operands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HmacOperation {
    New,
    SingleShot,
    BuiltinNew,
    BuiltinSingleShot,
}

fn digest_name(
    py: &PyToken,
    digest_bits: u64,
    constructors_bits: u64,
    error_bits: u64,
) -> Result<String, u64> {
    let result = unsafe {
        let objects = [
            NativeObjectOwner::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(digest_bits)),
            NativeObjectOwner::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(constructors_bits),
            ),
            NativeObjectOwner::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(error_bits)),
        ];
        if objects.iter().any(|object| object.as_ptr().is_null()) {
            Err(MoltObject::none().bits())
        } else {
            let digest = objects[0].as_ptr();
            let name = if strings::PyUnicode_Check(digest) != 0 {
                digest
            } else {
                mapping::PyDict_GetItemWithError(objects[1].as_ptr(), digest)
            };
            if name.is_null() {
                if errors::PyErr_Occurred().is_null() {
                    errors::PyErr_Format(
                        objects[2].as_ptr(),
                        c"Unsupported digestmod %R".as_ptr(),
                        digest,
                    );
                }
                Err(MoltObject::none().bits())
            } else {
                let name = NativeObjectOwner::from_borrowed(name);
                let mut length = 0;
                let text = strings::PyUnicode_AsUTF8AndSize(name.as_ptr(), &raw mut length);
                if text.is_null() {
                    Err(MoltObject::none().bits())
                } else {
                    let bytes = std::slice::from_raw_parts(text.cast::<u8>(), length as usize);
                    Ok(String::from_utf8_lossy(bytes).into_owned())
                }
            }
        }
    };
    retire_native_reference(|| ());
    if exception_pending(py) {
        Err(MoltObject::none().bits())
    } else {
        result
    }
}

fn builtin_digest_name(
    py: &PyToken,
    digest_bits: u64,
    error_bits: u64,
) -> Result<&'static str, u64> {
    let result = unsafe {
        (|| {
            let digest = NativeObjectOwner::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(digest_bits),
            );
            let error = NativeObjectOwner::from_owned(
                GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(error_bits),
            );
            if digest.as_ptr().is_null() || error.as_ptr().is_null() {
                return Err(MoltObject::none().bits());
            }
            if strings::PyUnicode_Check(digest.as_ptr()) != 0 {
                let lookup = |value| {
                    let text = strings::PyUnicode_AsUTF8(value);
                    if text.is_null() {
                        return Err(MoltObject::none().bits());
                    }
                    Ok(builtin_hmac_name(
                        std::ffi::CStr::from_ptr(text).to_str().unwrap(),
                    ))
                };
                if let Some(name) = lookup(digest.as_ptr())? {
                    return Ok(name);
                }
                // This is an actual overridable lower() call, only after exact
                // builtin lookup misses, and before either buffer is acquired.
                let method =
                    NativeObjectOwner::from_owned(strings::PyUnicode_FromString(c"lower".as_ptr()));
                if method.as_ptr().is_null() {
                    return Err(MoltObject::none().bits());
                }
                let lower = NativeObjectOwner::from_owned(object::PyObject_CallMethodNoArgs(
                    digest.as_ptr(),
                    method.as_ptr(),
                ));
                if lower.as_ptr().is_null() {
                    return Err(MoltObject::none().bits());
                }
                if let Some(name) = lookup(lower.as_ptr())? {
                    return Ok(name);
                }
            }
            errors::PyErr_Format(
                error.as_ptr(),
                c"unsupported hash type: %R".as_ptr(),
                digest.as_ptr(),
            );
            Err(MoltObject::none().bits())
        })()
    };
    retire_native_reference(|| ());
    if exception_pending(py) {
        Err(MoltObject::none().bits())
    } else {
        result
    }
}

fn build_hmac_handle(
    py: &PyToken,
    key: &ByteInput<'_>,
    name: &str,
    error_bits: u64,
) -> Result<HmacHandle, u64> {
    // All algorithms use the existing hash codec. Metadata is read from that
    // same state; custom digest objects are the stdlib's separate PEP 247 role.
    let values: [Option<OwnedRuntimeValue<'_>>; 12] = std::array::from_fn(|_| None);
    let mut inputs = [None, None, None];
    let mut inner = build_hash_handle(py, name, &values, &mut inputs, Some(error_bits))?;
    if inner.is_xof {
        return Err(raise_exception::<u64>(
            py,
            BuiltinExc::ValueError,
            "no reason supplied",
        ));
    }
    let mut outer = inner.clone();
    let digest_size = inner.digest_size;
    let block_size = inner.block_size;
    let raw_key = unsafe { key.bytes(py) };
    let reduced;
    let key_bytes = if raw_key.len() > block_size {
        let mut key_hash = inner.clone();
        key_hash.update(raw_key);
        reduced = key_hash
            .finalize_bytes(None)
            .map_err(|_| molt_runtime_core::rt_raise_class(py, error_bits, "unsupported digest"))?;
        reduced.as_slice()
    } else {
        raw_key
    };
    let mut inner_pad = vec![0x36; block_size];
    let mut outer_pad = vec![0x5c; block_size];
    for (index, byte) in key_bytes.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }
    inner.update(&inner_pad);
    outer.update(&outer_pad);
    Ok(HmacHandle {
        inner,
        outer,
        digest_size,
        block_size,
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hmac_new(
    key_bits: u64,
    msg_bits: u64,
    digest_bits: u64,
    constructors_bits: u64,
    operation_bits: u64,
    error_bits: u64,
    digestmod_present_bits: u64,
) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let operation = match to_i64(obj_from_bits(operation_bits)) {
            Some(0) => HmacOperation::New,
            Some(1) => HmacOperation::SingleShot,
            Some(2) => HmacOperation::BuiltinNew,
            Some(3) => HmacOperation::BuiltinSingleShot,
            _ => {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::SystemError,
                    "invalid HMAC operation role",
                );
            }
        };
        let Some(digestmod_present) = obj_from_bits(digestmod_present_bits).as_bool() else {
            return raise_exception::<u64>(
                _py,
                BuiltinExc::SystemError,
                "invalid HMAC digestmod presence",
            );
        };
        let error = OwnedRuntimeValue::retain(_py, error_bits);
        let builtin = matches!(
            operation,
            HmacOperation::BuiltinNew | HmacOperation::BuiltinSingleShot
        );
        let single = matches!(
            operation,
            HmacOperation::SingleShot | HmacOperation::BuiltinSingleShot
        );
        let builtin_name = if builtin {
            if !digestmod_present {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "new() missing 1 required argument 'digestmod'",
                );
            }
            match builtin_digest_name(_py, digest_bits, error.bits()) {
                Ok(name) => Some(name),
                Err(bits) => return bits,
            }
        } else {
            None
        };
        // Native Clinic retires key then msg; builtin singleshot retires msg
        // then key. Array field order carries that protocol on every exit.
        let key_index = usize::from(builtin && single);
        let msg_index = 1 - key_index;
        let mut inputs = [None, None];
        inputs[key_index] = Some(
            match if builtin {
                hash_input(_py, key_bits)
            } else {
                clinic_input(
                    _py,
                    key_bits,
                    if single { "hmac_digest" } else { "hmac_new" },
                    "key",
                )
            } {
                Ok(input) => input,
                Err(bits) => return bits,
            },
        );
        if builtin
            && unsafe { inputs[key_index].as_ref().unwrap().bytes(_py) }.len() > u32::MAX as usize
        {
            if single {
                drop(inputs);
            }
            return raise_exception::<u64>(
                _py,
                BuiltinExc::OverflowError,
                "key length exceeds 4294967295",
            );
        }
        if single {
            inputs[msg_index] = Some(
                match if builtin {
                    hash_input(_py, msg_bits)
                } else {
                    clinic_input(_py, msg_bits, "hmac_digest", "msg")
                } {
                    Ok(input) => input,
                    Err(bits) => return bits,
                },
            );
        }
        if builtin {
            if single
                && unsafe { inputs[msg_index].as_ref().unwrap().bytes(_py) }.len()
                    > u32::MAX as usize
            {
                drop(inputs);
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::OverflowError,
                    "message length exceeds 4294967295",
                );
            }
        } else {
            for (input, name) in inputs.iter().zip(["key", "msg"]) {
                if input
                    .as_ref()
                    .is_some_and(|input| unsafe { input.bytes(_py) }.len() > i32::MAX as usize)
                {
                    return raise_exception::<u64>(
                        _py,
                        BuiltinExc::OverflowError,
                        &format!("{name} is too long."),
                    );
                }
            }
            if !digestmod_present {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "Missing required parameter 'digestmod'.",
                );
            }
        }
        let native_name;
        let name = match builtin_name {
            Some(name) => name,
            None => {
                native_name = match digest_name(_py, digest_bits, constructors_bits, error.bits()) {
                    Ok(name) => name,
                    Err(bits) => return bits,
                };
                native_name.split('\0').next().unwrap()
            }
        };
        let mut handle =
            match build_hmac_handle(_py, inputs[key_index].as_ref().unwrap(), name, error.bits()) {
                Ok(handle) => handle,
                Err(bits) => return bits,
            };
        if builtin && !single {
            drop(inputs[key_index].take());
        }
        if !single && !obj_from_bits(msg_bits).is_none() {
            let message = match hash_input(_py, msg_bits) {
                Ok(input) => input,
                Err(bits) => return bits,
            };
            handle.inner.update(unsafe { message.bytes(_py) });
        } else if let Some(message) = &inputs[msg_index] {
            handle.inner.update(unsafe { message.bytes(_py) });
        }
        if single {
            let output = match handle.finalize_bytes() {
                Ok(output) => output,
                Err(_) => {
                    return raise_exception::<u64>(
                        _py,
                        BuiltinExc::ValueError,
                        "no reason supplied",
                    );
                }
            };
            if builtin {
                drop(inputs);
            }
            let pointer = alloc_bytes(_py, &output);
            if pointer.is_null() {
                return raise_exception::<u64>(_py, BuiltinExc::MemoryError, "out of memory");
            }
            return MoltObject::from_ptr(pointer).bits();
        }
        let name = handle.inner.name;
        let digest_size = handle.digest_size;
        let block_size = handle.block_size;
        let handle = unsafe { OwnedRuntimeValue::from_owned_bits(_py, new_hmac_owner(handle)) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let name = unsafe { OwnedRuntimeValue::from_owned_bits(_py, rt_string_from(name)) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let digest_size = unsafe {
            OwnedRuntimeValue::from_owned_bits(_py, int_bits_from_i64(_py, digest_size as i64))
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let block_size = unsafe {
            OwnedRuntimeValue::from_owned_bits(_py, int_bits_from_i64(_py, block_size as i64))
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        rt_tuple(&[
            handle.bits(),
            name.bits(),
            digest_size.bits(),
            block_size.bits(),
        ])
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_hmac_update(handle_bits: u64, data_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(owner) = hmac_owner(handle_bits) else {
            return raise_exception::<u64>(_py, BuiltinExc::TypeError, "invalid hmac handle");
        };
        let data = match hash_input(_py, data_bits) {
            Ok(slice) => slice,
            Err(bits) => return bits,
        };
        {
            let mut handle = owner.lock().unwrap_or_else(|poison| poison.into_inner());
            handle.inner.update(unsafe { data.bytes(_py) });
        }
        MoltObject::none().bits()
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_hmac_copy(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(owner) = hmac_owner(handle_bits) else {
            return raise_exception::<u64>(_py, BuiltinExc::TypeError, "invalid hmac handle");
        };
        let copy = owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        new_hmac_owner(copy)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_hmac_digest(handle_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let Some(owner) = hmac_owner(handle_bits) else {
            return raise_exception::<u64>(_py, BuiltinExc::TypeError, "invalid hmac handle");
        };
        let handle = owner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        let out = match handle.finalize_bytes() {
            Ok(bytes) => bytes,
            Err(_) => {
                return raise_exception::<u64>(_py, BuiltinExc::ValueError, "no reason supplied");
            }
        };
        let ptr = alloc_bytes(_py, &out);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, BuiltinExc::MemoryError, "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_compare_digest(a_bits: u64, b_bits: u64) -> u64 {
    use molt_cpython_abi::{
        abi_types,
        api::{buffer, errors, strings},
        bridge::GLOBAL_BRIDGE,
    };
    use molt_runtime_capi::{
        NativeObjectOwner, PyBUF_SIMPLE, RuntimeBufferLease, acquire_buffer_from_object,
        buffer_bytes, exact_bytes,
    };
    use subtle::ConstantTimeEq;

    molt_runtime_core::with_gil_entry!(_py, {
        let is_string = |bits| {
            obj_from_bits(bits)
                .as_ptr()
                .is_some_and(|ptr| unsafe { object_type_id(ptr) == TYPE_ID_STRING })
        };
        if is_string(a_bits) && is_string(b_bits) {
            // Unicode is immutable; these views borrow the call's operands.
            // No exporter callback or owned string copy is needed here.
            let a = unsafe { molt_runtime_core::rt_string_as_bytes(a_bits) }.unwrap();
            let b = unsafe { molt_runtime_core::rt_string_as_bytes(b_bits) }.unwrap();
            if !a.is_ascii() || !b.is_ascii() {
                return raise_exception::<u64>(
                    _py,
                    BuiltinExc::TypeError,
                    "comparing strings with non-ASCII characters is not supported",
                );
            }
            return MoltObject::from_bool(bool::from(a.ct_eq(b))).bits();
        }
        let exact_a = match unsafe { exact_bytes(_py, a_bits) } {
            Ok(bytes) => bytes.is_some(),
            Err(_) => return MoltObject::none().bits(),
        };
        let exact_b = match unsafe { exact_bytes(_py, b_bits) } {
            Ok(bytes) => bytes.is_some(),
            Err(_) => return MoltObject::none().bits(),
        };
        if exact_a && exact_b {
            // Admission is semantic. Borrow only after both class queries;
            // immutable exact bytes need no export or master allocation.
            let a = unsafe { molt_runtime_core::rt_bytes_as_slice(a_bits) }.unwrap();
            let b = unsafe { molt_runtime_core::rt_bytes_as_slice(b_bits) }.unwrap();
            return MoltObject::from_bool(bool::from(a.ct_eq(b))).bits();
        }
        unsafe {
            let objects = [
                NativeObjectOwner::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(a_bits)),
                NativeObjectOwner::from_owned(GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(b_bits)),
            ];
            if objects.iter().any(|owner| owner.as_ptr().is_null()) {
                return MoltObject::none().bits();
            }
            if strings::PyUnicode_Check(objects[0].as_ptr()) != 0
                && strings::PyUnicode_Check(objects[1].as_ptr()) != 0
            {
                if strings::PyUnicode_IS_ASCII(objects[0].as_ptr()) == 0
                    || strings::PyUnicode_IS_ASCII(objects[1].as_ptr()) == 0
                {
                    return raise_exception::<u64>(
                        _py,
                        BuiltinExc::TypeError,
                        "comparing strings with non-ASCII characters is not supported",
                    );
                }
                let mut lengths = [0, 0];
                let pointers = [
                    strings::PyUnicode_AsUTF8AndSize(objects[0].as_ptr(), &mut lengths[0]),
                    strings::PyUnicode_AsUTF8AndSize(objects[1].as_ptr(), &mut lengths[1]),
                ];
                if pointers.iter().any(|ptr| ptr.is_null()) {
                    return MoltObject::none().bits();
                }
                let a = std::slice::from_raw_parts(pointers[0].cast::<u8>(), lengths[0] as usize);
                let b = std::slice::from_raw_parts(pointers[1].cast::<u8>(), lengths[1] as usize);
                return MoltObject::from_bool(bool::from(a.ct_eq(b))).bits();
            }
            if buffer::PyObject_CheckBuffer(objects[0].as_ptr()) == 0
                && buffer::PyObject_CheckBuffer(objects[1].as_ptr()) == 0
            {
                molt_runtime_capi::retire_native_reference(|| {
                    errors::PyErr_Format(
                        (&raw mut abi_types::PyExc_TypeError).cast(),
                        c"unsupported operand types(s) or combination of types: '%.100s' and '%.100s'".as_ptr(),
                        (*(*objects[0].as_ptr()).ob_type).tp_name,
                        (*(*objects[1].as_ptr()).ob_type).tp_name,
                    );
                });
                return MoltObject::none().bits();
            }
            // Array element retirement is forward: a precedes b, including
            // b's failure and rank refusal. Never infer buffer eligibility
            // from a physical payload or retain raw slices across acquisition.
            let mut inputs: [Option<RuntimeBufferLease>; 2] = [None, None];
            for index in 0..2 {
                let lease = match acquire_buffer_from_object(objects[index].as_ptr(), PyBUF_SIMPLE)
                {
                    Ok(lease) => lease,
                    Err(_) => return MoltObject::none().bits(),
                };
                inputs[index] = Some(lease);
                if (*inputs[index].as_ref().unwrap().descriptor()).ndim > 1 {
                    return raise_exception::<u64>(
                        _py,
                        BuiltinExc::BufferError,
                        "Buffer must be single dimension",
                    );
                }
            }
            let a = inputs[0].as_ref().unwrap();
            let b = inputs[1].as_ref().unwrap();
            // GetBuffer's admitted spans are consumed without Python reentry.
            let result = bool::from(buffer_bytes(a).ct_eq(buffer_bytes(b)));
            drop(inputs);
            MoltObject::from_bool(result).bits()
        }
    })
}
