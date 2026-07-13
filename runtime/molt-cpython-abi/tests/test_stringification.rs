//! Mask-proof tests for `PyObject_Str` / `PyObject_Repr` (ledger F4 priority row,
//! `typeobj.rs:1907/1915`). Before the fix both returned the literal
//! `"<molt object>"` for EVERY object (zero-dispatch theater), corrupting `%S`
//! `PyErr_Format`, `PyUnicode_FromFormat`, and every dtype/array string path.
//!
//! These tests install a small but faithful runtime backend (real `alloc_str` /
//! `str_data` / `classify_heap`) so a native `int`/`str` round-trips through the
//! runtime str primitive, and construct genuine *foreign* type objects with
//! `tp_str` / `tp_repr` slots to prove slot dispatch. No result may ever be
//! `"<molt object>"`.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{PyObject, PyTypeObject};
use molt_cpython_abi::hooks::RuntimeHooks;
use std::os::raw::c_char;
use std::ptr;

unsafe extern "C" {
    fn molt_capi_unicode_from_format_probe(
        temporary_heap_allocations: *mut usize,
        heap_allocation_limit: usize,
        format: *const c_char,
        ...
    ) -> *mut PyObject;
}

// ── Faithful mini runtime backend: real native strings ───────────────────────
// Each interned str handle is a genuine `TAG_PTR` `MoltObject` over a leaked
// byte buffer, so `classify_heap` -> Str, `str_data` -> the bytes, and
// `handle_to_pyobj` stamps `ob_type == &PyUnicode_Type`.

unsafe extern "C" fn fake_classify_heap(bits: u64) -> u8 {
    use molt_cpython_abi::abi_types::MoltTypeTag;
    if support::fake_strings::contains(bits) {
        MoltTypeTag::Str as u8
    } else {
        MoltTypeTag::Other as u8
    }
}

fn install() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.classify_heap = fake_classify_heap;
    support::fake_strings::wire(&mut hooks);
    unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

/// Read the UTF-8 bytes backing a Molt-native `str` result.
unsafe fn read_native_str(py: *mut PyObject) -> Vec<u8> {
    let mut len = 0;
    let data = unsafe { molt_cpython_abi::api::strings::PyUnicode_AsUTF8AndSize(py, &raw mut len) };
    assert!(!data.is_null(), "result must expose valid Unicode storage");
    assert!(len >= 0);
    unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len as usize) }.to_vec()
}

// ── Foreign type-object scaffolding ──────────────────────────────────────────

fn make_type(
    name: *const c_char,
    tp_str: Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>,
    tp_repr: Option<unsafe extern "C" fn(*mut PyObject) -> *mut PyObject>,
) -> *mut PyTypeObject {
    let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    ty.ob_base.ob_base.ob_refcnt = 1;
    ty.tp_name = name;
    ty.tp_str = tp_str;
    ty.tp_repr = tp_repr;
    Box::into_raw(ty)
}

/// A genuine foreign instance: a real dereferenceable `PyObject` whose `ob_type`
/// is a foreign type. NOT bridge-registered, so `pyobj_to_handle` -> None (the
/// foreign path).
fn make_instance(ty: *mut PyTypeObject) -> *mut PyObject {
    let obj = Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: ty,
    });
    Box::into_raw(obj)
}

unsafe extern "C" fn foreign_str(_o: *mut PyObject) -> *mut PyObject {
    unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"FOREIGN_STR".as_ptr()) }
}

unsafe extern "C" fn foreign_repr(_o: *mut PyObject) -> *mut PyObject {
    unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"foreign_repr()".as_ptr()) }
}

unsafe extern "C" fn foreign_str_returns_int(_o: *mut PyObject) -> *mut PyObject {
    // A slot that lies and returns a non-str — CPython raises TypeError.
    unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) }
}

unsafe extern "C" fn foreign_str_raises(_o: *mut PyObject) -> *mut PyObject {
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast::<PyObject>(),
            c"stringification failed".as_ptr(),
        );
    }
    ptr::null_mut()
}

unsafe extern "C" fn recursive_repr(o: *mut PyObject) -> *mut PyObject {
    unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(o) }
}

// ===========================================================================
// Native scalars route through the runtime str/repr primitive (no theater).
// ===========================================================================

#[test]
fn native_int_str_is_the_decimal_digits() {
    install();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(py) };
    assert!(!s.is_null(), "str(42) must not be NULL");
    assert_eq!(unsafe { read_native_str(s) }, b"42");
}

#[test]
fn native_int_repr_is_the_decimal_digits() {
    install();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(-17) };
    let r = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(py) };
    assert!(!r.is_null());
    assert_eq!(unsafe { read_native_str(r) }, b"-17");
}

#[test]
fn native_float_bool_and_none_are_exact() {
    install();
    let float = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(3.5) };
    let true_obj = (&raw mut molt_cpython_abi::abi_types::Py_True).cast::<PyObject>();
    let false_obj = (&raw mut molt_cpython_abi::abi_types::Py_False).cast::<PyObject>();
    let none_obj = &raw mut molt_cpython_abi::abi_types::Py_None;
    for (obj, expected) in [
        (float, &b"3.5"[..]),
        (true_obj, &b"True"[..]),
        (false_obj, &b"False"[..]),
        (none_obj, &b"None"[..]),
    ] {
        let str_obj = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(obj) };
        let repr_obj = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(obj) };
        assert_eq!(unsafe { read_native_str(str_obj) }, expected);
        assert_eq!(unsafe { read_native_str(repr_obj) }, expected);
    }
}

#[test]
fn native_str_str_is_identity_passthrough() {
    install();
    let s = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"hello".as_ptr()) };
    assert!(!s.is_null());
    // str(s) is s — same object (CPython PyUnicode_CheckExact fast path).
    let out = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(s) };
    assert_eq!(out, s, "str(str) must return the same object");
}

#[test]
fn native_str_repr_is_quoted() {
    install();
    let s = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"hi".as_ptr()) };
    let r = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(s) };
    assert!(!r.is_null());
    assert_eq!(unsafe { read_native_str(r) }, b"'hi'");
}

// ===========================================================================
// Foreign objects dispatch their OWN tp_str / tp_repr slots.
// ===========================================================================

#[test]
fn foreign_object_str_dispatches_tp_str() {
    install();
    let ty = make_type(c"Widget".as_ptr(), Some(foreign_str), Some(foreign_repr));
    let inst = make_instance(ty);
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(inst) };
    assert!(!s.is_null());
    let bytes = unsafe { read_native_str(s) };
    assert_eq!(bytes, b"FOREIGN_STR");
    assert_ne!(
        bytes, b"<molt object>",
        "must not be the old theater constant"
    );
}

#[test]
fn foreign_object_repr_dispatches_tp_repr() {
    install();
    let ty = make_type(c"Widget".as_ptr(), Some(foreign_str), Some(foreign_repr));
    let inst = make_instance(ty);
    let r = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(inst) };
    assert!(!r.is_null());
    assert_eq!(unsafe { read_native_str(r) }, b"foreign_repr()");
}

#[test]
fn foreign_str_falls_back_to_repr_when_tp_str_null() {
    install();
    // tp_str == NULL: CPython PyObject_Str falls back to PyObject_Repr -> tp_repr.
    let ty = make_type(c"Widget".as_ptr(), None, Some(foreign_repr));
    let inst = make_instance(ty);
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(inst) };
    assert!(!s.is_null());
    assert_eq!(unsafe { read_native_str(s) }, b"foreign_repr()");
}

#[test]
fn foreign_repr_default_is_type_name_and_address() {
    install();
    // tp_repr == NULL: CPython default "<%s object at %p>".
    let ty = make_type(c"gadget".as_ptr(), None, None);
    let inst = make_instance(ty);
    let r = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(inst) };
    assert!(!r.is_null());
    let bytes = unsafe { read_native_str(r) };
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("<gadget object at 0x"), "got {text:?}");
    assert!(text.ends_with('>'), "got {text:?}");
    assert_ne!(text, "<molt object>");
}

#[test]
fn foreign_str_slot_returning_non_string_raises_typeerror() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let ty = make_type(c"Liar".as_ptr(), Some(foreign_str_returns_int), None);
    let inst = make_instance(ty);
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(inst) };
    assert!(
        s.is_null(),
        "a non-str tp_str result must fail, not pass through"
    );
    let err = unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() };
    assert!(
        !err.is_null(),
        "must set TypeError on non-string __str__ result"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn foreign_str_slot_exception_propagates_without_placeholder() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let ty = make_type(c"Raises".as_ptr(), Some(foreign_str_raises), None);
    let inst = make_instance(ty);
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(inst) };
    assert!(result.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn recursive_repr_raises_instead_of_overflowing_or_fabricating() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let ty = make_type(c"Recursive".as_ptr(), None, Some(recursive_repr));
    let inst = make_instance(ty);
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(inst) };
    assert!(result.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn null_object_str_is_angle_null() {
    install();
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(ptr::null_mut()) };
    assert!(!s.is_null());
    assert_eq!(unsafe { read_native_str(s) }, b"<NULL>");
}

// ===========================================================================
// One PyUnicode_FromFormatV-compatible authority for every variadic consumer.
// ===========================================================================

#[test]
fn unicode_formatter_applies_integer_lengths_width_and_precision() {
    install();
    let out = unsafe {
        molt_cpython_abi::api::errors::PyUnicode_FromFormat(
            c"%05d|%i|%-6u|%08.4x|%ld|%llo|%zu|%td|%jd|%X".as_ptr(),
            -12_i32,
            3_i32,
            7_u32,
            42_u32,
            -8 as std::os::raw::c_long,
            9_u64,
            123_usize,
            -4_isize,
            -99_i64,
            0xab_u32,
        )
    };
    assert!(!out.is_null());
    assert_eq!(
        unsafe { read_native_str(out) },
        b"-0012|3|7     |0000002a|-8|11|123|-4|-99|AB"
    );
}

#[test]
fn unicode_formatter_handles_utf8_wide_unicode_and_v_fallbacks() {
    install();
    let unicode = unsafe {
        molt_cpython_abi::api::strings::PyUnicode_FromStringAndSize(
            "éxy".as_ptr().cast(),
            "éxy".len() as isize,
        )
    };
    assert!(!unicode.is_null());
    let wide: [libc::wchar_t; 2] = [0x03a9 as libc::wchar_t, 0];
    let out = unsafe {
        molt_cpython_abi::api::errors::PyUnicode_FromFormat(
            c"%.2s|%ls|%6.3U|%V|%V|%*.*s".as_ptr(),
            c"éclair".as_ptr(),
            wide.as_ptr(),
            unicode,
            unicode,
            c"ignored".as_ptr(),
            ptr::null_mut::<PyObject>(),
            c"fallback".as_ptr(),
            5_i32,
            2_i32,
            c"abcd".as_ptr(),
        )
    };
    assert!(!out.is_null());
    assert_eq!(
        unsafe { read_native_str(out) },
        "é|Ω|   éxy|éxy|fallback|   ab".as_bytes()
    );
}

#[test]
fn unicode_formatter_replacement_decoder_matches_terminal_and_middle_errors() {
    install();
    let incomplete = [0xe2_u8, 0x82, 0];
    let out = unsafe {
        molt_cpython_abi::api::errors::PyUnicode_FromFormat(
            c"%s".as_ptr(),
            incomplete.as_ptr().cast::<c_char>(),
        )
    };
    assert!(!out.is_null());
    assert_eq!(unsafe { read_native_str(out) }, "\u{fffd}".as_bytes());

    let precision_cut = [0xe2_u8, 0x82, 0xac, 0];
    let out = unsafe {
        molt_cpython_abi::api::errors::PyUnicode_FromFormat(
            c"%.2s".as_ptr(),
            precision_cut.as_ptr().cast::<c_char>(),
        )
    };
    assert!(!out.is_null());
    assert_eq!(unsafe { read_native_str(out) }, b"");

    let invalid_middle = [0xe2_u8, b'(', 0xa1, 0];
    let out = unsafe {
        molt_cpython_abi::api::errors::PyUnicode_FromFormat(
            c"%s".as_ptr(),
            invalid_middle.as_ptr().cast::<c_char>(),
        )
    };
    assert!(!out.is_null());
    assert_eq!(
        unsafe { read_native_str(out) },
        "\u{fffd}(\u{fffd}".as_bytes()
    );
}

#[test]
fn formatter_inline_storage_avoids_heap_and_counts_boundary_spill() {
    install();
    let inline = std::ffi::CString::new("x".repeat(255)).unwrap();
    let mut allocations = usize::MAX;
    let out = unsafe {
        molt_capi_unicode_from_format_probe(&mut allocations, usize::MAX, inline.as_ptr())
    };
    assert!(!out.is_null());
    assert_eq!(allocations, 0, "255 bytes plus NUL must remain inline");
    assert_eq!(unsafe { read_native_str(out) }.len(), 255);

    let spill = std::ffi::CString::new("x".repeat(256)).unwrap();
    allocations = usize::MAX;
    let out = unsafe {
        molt_capi_unicode_from_format_probe(&mut allocations, usize::MAX, spill.as_ptr())
    };
    assert!(!out.is_null());
    assert_eq!(
        allocations, 1,
        "the first byte past inline capacity must spill once"
    );
    assert_eq!(unsafe { read_native_str(out) }.len(), 256);

    let invalid_middle = [0xe2_u8, b'(', 0xa1, 0];
    allocations = usize::MAX;
    let out = unsafe {
        molt_capi_unicode_from_format_probe(
            &mut allocations,
            usize::MAX,
            c"%s".as_ptr(),
            invalid_middle.as_ptr().cast::<c_char>(),
        )
    };
    assert!(!out.is_null());
    assert_eq!(
        allocations, 0,
        "short cold-path replacement storage must also remain inline"
    );

    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    allocations = usize::MAX;
    let out = unsafe { molt_capi_unicode_from_format_probe(&mut allocations, 256, spill.as_ptr()) };
    assert!(
        out.is_null(),
        "a denied inline-to-heap spill must fail closed"
    );
    assert_eq!(
        allocations, 0,
        "a failed spill must not count an allocation"
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_MemoryError).cast::<PyObject>(),
            )
        },
        1
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn unicode_formatter_supports_object_and_type_conversions() {
    install();
    let ty = make_type(
        c"pkg.Widget".as_ptr(),
        Some(foreign_str),
        Some(foreign_repr),
    );
    unsafe {
        (*ty).ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    }
    let inst = make_instance(ty);
    let out = unsafe {
        molt_cpython_abi::api::errors::PyUnicode_FromFormat(
            c"%S|%R|%A|%T|%#T|%N|%#N|%c|%p|%%".as_ptr(),
            inst,
            inst,
            inst,
            inst,
            inst,
            ty.cast::<PyObject>(),
            ty.cast::<PyObject>(),
            0x03a9_i32,
            inst.cast::<std::ffi::c_void>(),
        )
    };
    assert!(!out.is_null());
    let rendered = String::from_utf8(unsafe { read_native_str(out) }).unwrap();
    let parts: Vec<&str> = rendered.split('|').collect();
    assert_eq!(
        &parts[..8],
        &[
            "FOREIGN_STR",
            "foreign_repr()",
            "foreign_repr()",
            "pkg.Widget",
            "pkg:Widget",
            "pkg.Widget",
            "pkg:Widget",
            "\u{03a9}",
        ]
    );
    assert!(parts[8].starts_with("0x"), "pointer was {0:?}", parts[8]);
    assert_eq!(
        usize::from_str_radix(&parts[8][2..], 16).unwrap(),
        inst as usize
    );
    assert_eq!(parts[9], "%");
}

#[test]
fn negative_star_precision_is_zero_for_every_string_conversion() {
    install();
    let unicode = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"abcd".as_ptr()) };
    assert!(!unicode.is_null());
    let wide: [libc::wchar_t; 5] = [b'w' as _, b'i' as _, b'd' as _, b'e' as _, 0];
    let ty = make_type(
        c"pkg.Widget".as_ptr(),
        Some(foreign_str),
        Some(foreign_repr),
    );
    unsafe {
        (*ty).ob_base.ob_base.ob_type = &raw mut molt_cpython_abi::abi_types::PyType_Type;
    }
    let inst = make_instance(ty);
    let type_refcount = unsafe { (*ty).ob_base.ob_base.ob_refcnt };
    let out = unsafe {
        molt_cpython_abi::api::errors::PyUnicode_FromFormat(
            c"%5.*s|%5.*ls|%5.*U|%5.*V|%5.*S|%5.*R|%5.*A|%5.*T|%5.*N".as_ptr(),
            -1_i32,
            c"abcd".as_ptr(),
            -1_i32,
            wide.as_ptr(),
            -1_i32,
            unicode,
            -1_i32,
            unicode,
            c"unused".as_ptr(),
            -1_i32,
            inst,
            -1_i32,
            inst,
            -1_i32,
            inst,
            -1_i32,
            inst,
            -1_i32,
            ty.cast::<PyObject>(),
        )
    };
    assert!(!out.is_null());
    assert_eq!(
        String::from_utf8(unsafe { read_native_str(out) }).unwrap(),
        ["     "; 9].join("|")
    );
    assert_eq!(
        unsafe { (*ty).ob_base.ob_base.ob_refcnt },
        type_refcount,
        "%T must retain and release the observed type exactly once"
    );
}

#[cfg(all(windows, target_pointer_width = "64"))]
#[test]
fn pointer_format_preserves_windows_printf_width_and_case() {
    install();
    let pointer = 0xabcdefusize as *mut std::ffi::c_void;
    let out =
        unsafe { molt_cpython_abi::api::errors::PyUnicode_FromFormat(c"%p".as_ptr(), pointer) };
    assert!(!out.is_null());
    assert_eq!(unsafe { read_native_str(out) }, b"0x0000000000ABCDEF");
}

#[test]
fn signed_integer_max_precision_overflow_fails_before_allocation() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let format = std::ffi::CString::new(format!("%.{}d", isize::MAX)).unwrap();
    let out =
        unsafe { molt_cpython_abi::api::errors::PyUnicode_FromFormat(format.as_ptr(), -1_i32) };
    assert!(out.is_null());
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast::<PyObject>(),
            )
        },
        1
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn lone_surrogate_c_is_known_utf8_storage_limit_and_fails_closed() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let out =
        unsafe { molt_cpython_abi::api::errors::PyUnicode_FromFormat(c"%c".as_ptr(), 0xd800_i32) };
    assert!(
        out.is_null(),
        "Molt must not fabricate U+FFFD or install invalid UTF-8 for a lone surrogate"
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>(),
            )
        },
        1,
        "known limit remains fail-closed until runtime strings gain code-point storage"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn pyerr_format_preserves_formatter_and_repr_errors() {
    install();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let result = unsafe {
        molt_cpython_abi::api::errors::PyErr_Format(
            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>(),
            c"invalid %Q".as_ptr(),
        )
    };
    assert!(result.is_null());
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>(),
            )
        },
        1,
        "invalid format must remain SystemError, not literal-format TypeError"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    let ty = make_type(c"Raises".as_ptr(), None, Some(foreign_str_raises));
    let inst = make_instance(ty);
    let result = unsafe {
        molt_cpython_abi::api::errors::PyErr_Format(
            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>(),
            c"repr: %R".as_ptr(),
            inst,
        )
    };
    assert!(result.is_null());
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast::<PyObject>(),
            )
        },
        1,
        "repr failure must survive PyErr_FormatV unchanged"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
