//! The runtime side of the bridge-owned Unicode construction transaction.

use super::*;

pub(super) unsafe extern "C" fn hook_unicode_new(len: usize, maxchar: u32) -> OwnedHandleResult {
    with_gil(|py| {
        if maxchar > 0x10ffff || len > isize::MAX as usize {
            crate::raise_exception::<()>(
                &py,
                "ValueError",
                "invalid Unicode construction size or maximum character",
            );
            return OwnedHandleResult::error();
        }
        let Some(capacity) = len.checked_mul(4) else {
            crate::builtins::exceptions::record_memory_error_without_allocation(&py);
            return OwnedHandleResult::error();
        };
        let ptr = crate::object::builders::alloc_inline_bytes_with_len(
            &py,
            capacity,
            crate::object::builders::InlineBytesKind::String,
        );
        if ptr.is_null() {
            return OwnedHandleResult::error();
        }
        unsafe {
            let data = crate::string_bytes(ptr) as *mut u8;
            std::ptr::write_bytes(data, 0, capacity + 1);
            *(ptr.cast::<usize>()) = len;
        }
        OwnedHandleResult::ok(MoltObject::from_ptr(ptr).bits())
    })
}

pub(super) unsafe extern "C" fn hook_unicode_commit(
    bits: u64,
    data: *const u8,
    len: usize,
) -> c_int {
    with_gil(|py| {
        let Some(ptr) = crate::obj_from_bits(bits)
            .as_ptr()
            .filter(|ptr| unsafe { crate::object_type_id(*ptr) == crate::TYPE_ID_STRING })
        else {
            crate::raise_exception::<()>(
                &py,
                "SystemError",
                "Unicode construction requires string storage",
            );
            return -1;
        };
        let capacity = unsafe { crate::object::object_payload_size(ptr) }
            .saturating_sub(std::mem::size_of::<usize>() + 1);
        if len > capacity || (len != 0 && data.is_null()) {
            crate::raise_exception::<()>(
                &py,
                "SystemError",
                "Unicode construction exceeded reserved storage",
            );
            return -1;
        }
        // The bridge's object-owned open state is the commit capability; this
        // primitive is never used to mutate a published immutable string.
        unsafe {
            let target = crate::string_bytes(ptr) as *mut u8;
            if len != 0 {
                std::ptr::copy_nonoverlapping(data, target, len);
            }
            *target.add(len) = 0;
            *(ptr.cast::<usize>()) = len;
        }
        crate::object::object_set_state(ptr, 0);
        crate::object::ops_string::utf8_cache_remove(&py, ptr as usize);
        0
    })
}

pub(super) unsafe extern "C" fn hook_unicode_encode(
    bits: u64,
    encoding: u64,
    errors: u64,
) -> OwnedHandleResult {
    owned_result_from_pending(crate::molt_string_encode(bits, encoding, errors))
}

#[cfg(test)]
mod tests {
    use molt_cpython_abi::{
        abi_types::*,
        api::{errors, numbers, object, refcount, strings, typeobj},
        bridge::GLOBAL_BRIDGE,
    };
    use std::ptr;

    unsafe fn runtime_text(op: *mut PyObject) -> Vec<u8> {
        assert!(!op.is_null());
        let bits = GLOBAL_BRIDGE.observed_handle_for_pyobj(op).unwrap().bits();
        let mut len = 0;
        let data = unsafe { (molt_cpython_abi::hooks::hooks_or_stubs().str_data)(bits, &mut len) };
        assert!(!data.is_null());
        unsafe { std::slice::from_raw_parts(data, len).to_vec() }
    }

    unsafe fn owned_bytes(op: *mut PyObject) -> Vec<u8> {
        assert!(!op.is_null());
        let mut data = ptr::null_mut();
        let mut len = 0;
        assert_eq!(
            unsafe { strings::PyBytes_AsStringAndSize(op, &mut data, &mut len) },
            0
        );
        let result =
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len as usize).to_vec() };
        unsafe { refcount::Py_DECREF(op) };
        result
    }

    #[test]
    fn c_unicode_construction_exports_commit_one_lossless_runtime_value_and_codec_policy() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::cpython_abi_hooks::register_cpython_hooks();
        unsafe { errors::PyErr_Clear() };
        crate::with_gil_entry_nopanic!(py, {
            unsafe {
                let text = strings::PyUnicode_New(4, 0x10ffff);
                assert!(!text.is_null());
                assert_eq!(strings::molt_capi_unicode_kind(text), 4);
                assert_eq!(strings::PyUnicode_GetLength(text), 4);
                assert_eq!(strings::PyUnicode_Check(text), 1);
                assert_eq!(strings::PyUnicode_CheckExact(text), 1);
                let data = strings::molt_capi_unicode_data(text).cast::<u32>();
                assert!(!data.is_null());
                // The same raw writes emitted by PyUnicode_WRITE on both headers.
                for (index, code) in [0xd800, 0xdfff, 0, 0x1f600].into_iter().enumerate() {
                    *data.add(index) = code;
                    assert_eq!(strings::PyUnicode_ReadChar(text, index as isize), code);
                }
                assert_eq!(*data.add(4), 0);
                let expected = b"\xed\xa0\x80\xed\xbf\xbf\0\xf0\x9f\x98\x80";
                assert_eq!(runtime_text(text), expected);
                let bits = GLOBAL_BRIDGE
                    .observed_handle_for_pyobj(text)
                    .unwrap()
                    .bits();
                assert_eq!(
                    crate::obj_from_bits(crate::molt_len(bits)).as_int(),
                    Some(4)
                );
                assert_eq!(strings::molt_capi_unicode_data(text).cast::<u32>(), data);
                assert!(strings::PyUnicode_AsUTF8AndSize(text, ptr::null_mut()).is_null());
                assert_eq!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_UnicodeEncodeError).cast()),
                    1
                );
                let pending = errors::PyErr_Occurred();
                assert_eq!(strings::molt_capi_unicode_data(text).cast::<u32>(), data);
                assert_eq!(strings::PyUnicode_ReadChar(text, 0), 0xd800);
                assert_eq!(
                    errors::PyErr_Occurred(),
                    pending,
                    "raw codepoint exports do not encode or replace errors"
                );
                errors::PyErr_Clear();
                assert_eq!(
                    owned_bytes(strings::PyUnicode_AsEncodedString(
                        text,
                        c"utf-8".as_ptr(),
                        c"surrogatepass".as_ptr()
                    )),
                    expected
                );
                assert_eq!(
                    owned_bytes(strings::PyUnicode_AsEncodedString(
                        text,
                        c"utf-8".as_ptr(),
                        c"backslashreplace".as_ptr()
                    )),
                    b"\\ud800\\udfff\0\xf0\x9f\x98\x80"
                );
                assert_eq!(
                    owned_bytes(strings::PyUnicode_AsEncodedString(
                        text,
                        c"ascii".as_ptr(),
                        c"replace".as_ptr()
                    )),
                    b"??\0?"
                );
                assert_eq!(
                    owned_bytes(strings::PyUnicode_AsEncodedString(
                        text,
                        c"ascii".as_ptr(),
                        c"ignore".as_ptr()
                    )),
                    b"\0"
                );
                assert!(
                    strings::PyUnicode_AsEncodedString(
                        text,
                        c"ascii".as_ptr(),
                        c"unknown-policy".as_ptr()
                    )
                    .is_null()
                );
                assert_eq!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_LookupError).cast()),
                    1
                );
                errors::PyErr_Clear();
                refcount::Py_DECREF(text);
                assert!(!crate::exception_pending(py));
            }
        });
    }

    #[test]
    fn c_unicode_copy_and_width_exports_feed_runtime_text_without_utf8_aliases() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::cpython_abi_hooks::register_cpython_hooks();
        unsafe { errors::PyErr_Clear() };
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let one = strings::PyUnicode_New(3, 0xff);
                assert_eq!(strings::molt_capi_unicode_kind(one), 1);
                assert_eq!(strings::PyUnicode_IS_ASCII(one), 0);
                assert_eq!(strings::molt_capi_unicode_maxchar(one), 0xff);
                refcount::Py_INCREF(one);
                assert_eq!(strings::PyUnicode_WriteChar(one, 0, b'X' as u32), -1);
                assert_eq!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_ValueError).cast()),
                    1
                );
                errors::PyErr_Clear();
                refcount::Py_DECREF(one);
                assert_eq!(strings::PyUnicode_WriteChar(one, 0, b'A' as u32), 0);
                assert_eq!(strings::PyUnicode_WriteChar(one, 1, 0xe9), 0);
                assert_eq!(strings::PyUnicode_WriteChar(one, 2, 0), 0);
                let two = strings::PyUnicode_New(4, 0xd800);
                assert_eq!(strings::molt_capi_unicode_kind(two), 2);
                strings::_PyUnicode_FastCopyCharacters(two, 0, one, 0, 3);
                assert_eq!(strings::PyUnicode_WriteChar(two, 3, 0xd800), 0);
                assert_eq!(runtime_text(two), b"A\xc3\xa9\0\xed\xa0\x80");
                assert_eq!(runtime_text(one), b"A\xc3\xa9\0");
                assert_eq!(strings::PyUnicode_WriteChar(one, 0, b'X' as u32), -1);
                assert_eq!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_ValueError).cast()),
                    1
                );
                errors::PyErr_Clear();
                assert_eq!(runtime_text(one), b"A\xc3\xa9\0");
                let first = strings::PyUnicode_AsUTF8AndSize(one, ptr::null_mut());
                assert!(!first.is_null());
                assert_eq!(
                    strings::PyUnicode_AsUTF8AndSize(one, ptr::null_mut()),
                    first
                );
                assert_ne!(
                    first.cast::<std::ffi::c_void>(),
                    strings::molt_capi_unicode_data(one).cast_const()
                );
                assert_eq!(strings::PyUnicode_ReadChar(one, 1), 0xe9);
                for op in [one, two] {
                    refcount::Py_DECREF(op);
                }
            }
        });
    }

    #[test]
    fn c_unicode_internal_names_and_numeric_parsing_preserve_error_text() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::cpython_abi_hooks::register_cpython_hooks();
        unsafe { errors::PyErr_Clear() };
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let units = [b'n' as u32, 0xd800, 0, b'm' as u32];
                let name = strings::PyUnicode_FromKindAndData(4, units.as_ptr().cast(), 4);
                let mut native_type: PyTypeObject = std::mem::zeroed();
                native_type.ob_base.ob_base.ob_refcnt = IMMORTAL_REFCNT;
                native_type.ob_base.ob_base.ob_type = &raw mut PyType_Type;
                native_type.tp_name = c"UnicodeOwner".as_ptr();
                native_type.tp_basicsize = std::mem::size_of::<PyObject>() as isize;
                native_type.tp_flags = Py_TPFLAGS_DEFAULT;
                let mut native = PyObject {
                    ob_refcnt: 1,
                    ob_type: &mut native_type,
                };
                let mut found = ptr::null_mut();
                assert_eq!(
                    object::PyObject_GetOptionalAttr(&mut native, name, &mut found),
                    0
                );
                assert!(found.is_null() && errors::PyErr_Occurred().is_null());
                assert!(object::PyObject_GetAttr(&mut native, name).is_null());
                let error = errors::PyErr_GetRaisedException();
                let error_owner = refcount::OwnedPyObject::from_owned(error);
                let rendered = typeobj::PyObject_Str(error);
                let rendered_owner = refcount::OwnedPyObject::from_owned(rendered);
                assert_eq!(
                    runtime_text(rendered),
                    b"'UnicodeOwner' object has no attribute 'n\xed\xa0\x80\0m'"
                );
                drop(rendered_owner);
                drop(error_owner);
                assert!(numbers::PyLong_FromUnicodeObject(name, 10).is_null());
                assert_eq!(
                    errors::PyErr_ExceptionMatches((&raw mut PyExc_ValueError).cast()),
                    1
                );
                errors::PyErr_Clear();
                refcount::Py_DECREF(name);
            }
        });
    }
}
