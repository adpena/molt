//! F1 mask-proof gates for the `PyArg_ParseTuple` format engine
//! (`molt_pyarg_parse_tuple_inner` + the `pyarg_variadic.c` shim).
//!
//! Physical tuples and integers are built through the C API, then passed to
//! the real variadic `PyArg_ParseTuple`. The shim and canonical format plan
//! account for output addresses end-to-end. Owned guards retain the arguments
//! and release them even when an assertion fails.
//!
//! The teeth target the two P0 memory-safety divergences and the theater/surplus
//! rows:
//!   * `errors.rs:512` — b/B/H width: the store must be EXACTLY the C width the
//!     caller declared (1/2 bytes), never a 4-byte `c_int` clobber of adjacent
//!     memory. Proven with guard bytes framing the target (load-bearing: the
//!     pre-fix 4-byte store zeroes the guards).
//!   * `errors.rs:556` — O!: the type object is READ (subtype check), never
//!     written through; the pre-fix grammar stored the object into the type
//!     slot, corrupting the type-object header. Proven with a sentinel type.
//!   * `errors.rs:536` — s/z/y: a non-str/bytes arg is a TypeError, not a
//!     fabricated empty string.
//!   * `errors.rs:580` — surplus positional args raise TypeError.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{PyObject, PyTypeObject};
use molt_cpython_abi::api::refcount::OwnedPyObject;
use std::ffi::{c_char, c_int, c_void};

fn install_hooks() {
    let mut hooks = support::stub_runtime_hooks();
    support::fake_runtime::wire_numeric(&mut hooks);
    hooks.numeric_identity_new = Some(parse_numeric_identity);
    hooks.target_python_minor = numeric_target_minor;
    support::prepare_runtime_class_abi_test_thread(hooks);
}

fn args_with(items: &[i64]) -> OwnedPyObject {
    let tuple = unsafe {
        OwnedPyObject::from_owned(molt_cpython_abi::api::sequences::PyTuple_New(
            items.len() as isize
        ))
    };
    assert!(!tuple.as_ptr().is_null());
    for (index, &value) in items.iter().enumerate() {
        let item = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLongLong(value) };
        assert!(!item.is_null());
        // The checked setter consumes the integer on success and failure.
        assert_eq!(
            unsafe {
                molt_cpython_abi::api::sequences::PyTuple_SetItem(
                    tuple.as_ptr(),
                    index as isize,
                    item,
                )
            },
            0
        );
    }
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    tuple
}

// The real variadic entry from the C shim (linked into this test binary). Rust
// stable can CALL a C variadic (only defining one needs nightly).
unsafe extern "C" {
    fn PyArg_ParseTuple(args: *mut PyObject, format: *const c_char, ...) -> c_int;
}

fn clear_err() {
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}
fn err_is(exc: *mut PyObject) -> bool {
    unsafe { molt_cpython_abi::api::errors::PyErr_ExceptionMatches(exc) == 1 }
}

fn args_for_object(object: *mut PyObject) -> OwnedPyObject {
    let args = unsafe {
        OwnedPyObject::from_owned(molt_cpython_abi::api::sequences::PyTuple_FromArray(
            &raw const object,
            1,
        ))
    };
    assert!(!args.as_ptr().is_null());
    args
}

#[repr(C)]
struct NumericOutput<T> {
    before: u64,
    value: T,
    after: u64,
}

fn parse_numeric<T: Copy + PartialEq + std::fmt::Debug>(
    args: &OwnedPyObject,
    code: u8,
    initial: T,
    expected: T,
    status: c_int,
) {
    let format = [code, 0];
    let mut output = NumericOutput {
        before: 0x1357_9bdf_2468_ace0,
        value: initial,
        after: 0xfedc_ba98_7654_3210,
    };
    assert_eq!(
        unsafe {
            PyArg_ParseTuple(
                args.as_ptr(),
                format.as_ptr().cast::<c_char>(),
                &raw mut output.value,
            )
        },
        status,
        "format {}",
        code as char
    );
    assert_eq!(output.value, expected, "format {}", code as char);
    assert_eq!(output.before, 0x1357_9bdf_2468_ace0);
    assert_eq!(output.after, 0xfedc_ba98_7654_3210);
}

#[test]
fn numeric_units_read_physical_values_without_runtime_adoption() {
    install_hooks();
    // A real foreign physical prefix has no adopted runtime identity. The
    // hook denial is per-thread because the binary installs one hook table.
    let mut integer = molt_cpython_abi::abi_types::PyLongObject {
        ob_base: PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut molt_cpython_abi::abi_types::PyLong_Type,
        },
        long_value: molt_cpython_abi::abi_types::PyLongValue {
            lv_tag: 1 << 3,
            ob_digit: [1000],
        },
    };
    DENY_NUMERIC_IDENTITY.with(|value| value.set(true));
    NUMERIC_ADOPTIONS.with(|value| value.set(0));
    let args = args_for_object((&raw mut integer).cast());
    parse_numeric(&args, b'h', 77i16, 1000, 1);
    parse_numeric(&args, b'H', 77u16, 1000, 1);
    parse_numeric(&args, b'i', 77i32, 1000, 1);
    parse_numeric(&args, b'I', 77u32, 1000, 1);
    parse_numeric(&args, b'l', 77 as std::ffi::c_long, 1000, 1);
    parse_numeric(&args, b'k', 77 as std::ffi::c_ulong, 1000, 1);
    parse_numeric(&args, b'L', 77i64, 1000, 1);
    parse_numeric(&args, b'K', 77u64, 1000, 1);
    parse_numeric(&args, b'n', 77isize, 1000, 1);
    parse_numeric(&args, b'f', 77f32, 1000.0, 1);
    parse_numeric(&args, b'd', 77f64, 1000.0, 1);
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    assert_eq!(NUMERIC_ADOPTIONS.with(std::cell::Cell::get), 0);
    drop(args);
    assert_eq!(integer.ob_base.ob_refcnt, 1);
}

#[test]
fn numeric_mask_units_keep_wide_low_bits_and_signed_units_preserve_errors() {
    install_hooks();
    // Rust's fixed-width integer arithmetic supplies the independent oracle;
    // inputs pass the actual C byte constructor, tuple and variadic shim.
    for minor in [12, 13, 14] {
        NUMERIC_TARGET_MINOR.with(|value| value.set(minor));
        for value in [
            (1i128 << 100) + 0x1234_5678_9abc_def0,
            -((1i128 << 100) + 0x1234_5678_9abc_def0),
        ] {
            let bytes = value.to_le_bytes();
            let integer = unsafe {
                OwnedPyObject::from_owned(molt_cpython_abi::api::numbers::_PyLong_FromByteArray(
                    bytes.as_ptr(),
                    bytes.len(),
                    1,
                    1,
                ))
            };
            assert!(!integer.as_ptr().is_null());
            let args = args_for_object(integer.as_ptr());
            parse_numeric(&args, b'B', 77u8, value as u8, 1);
            parse_numeric(&args, b'H', 77u16, value as u16, 1);
            parse_numeric(&args, b'I', 77u32, value as u32, 1);
            parse_numeric(
                &args,
                b'k',
                77 as std::ffi::c_ulong,
                value as std::ffi::c_ulong,
                1,
            );
            parse_numeric(&args, b'K', 77u64, value as u64, 1);
            parse_numeric(&args, b'd', 77f64, value as f64, 1);
            for code in [b'b', b'h', b'i', b'l', b'L', b'n'] {
                // Each failure must leave every output byte intact. A wide slot
                // is safe for all these output pointer widths on all targets.
                parse_numeric(&args, code, 77u64, 77, 0);
                assert!(err_is(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast()
                ));
                let expected = match code {
                    b'L' => "int too big to convert",
                    b'n' => "Python int too large to convert to C ssize_t",
                    _ => "Python int too large to convert to C long",
                };
                assert_eq!(
                    support::take_current_error_text().as_deref(),
                    Some(expected)
                );
            }
        }
    }
}

thread_local! {
    static NUMERIC_TARGET_MINOR: std::cell::Cell<i64> = const { std::cell::Cell::new(12) };
    static INDEX_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FLOAT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static INDEX_FAILS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static DENY_NUMERIC_IDENTITY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static NUMERIC_ADOPTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

unsafe extern "C" fn parse_numeric_identity(
    bits: u64,
) -> molt_cpython_abi::hooks::OwnedHandleResult {
    NUMERIC_ADOPTIONS.with(|calls| calls.set(calls.get() + 1));
    if DENY_NUMERIC_IDENTITY.with(std::cell::Cell::get) {
        molt_cpython_abi::hooks::OwnedHandleResult::error()
    } else {
        unsafe { support::fake_runtime::numeric_identity_new(bits) }
    }
}

unsafe extern "C" fn numeric_target_minor() -> i64 {
    NUMERIC_TARGET_MINOR.with(std::cell::Cell::get)
}
unsafe extern "C" fn parse_index(_object: *mut PyObject) -> *mut PyObject {
    INDEX_CALLS.with(|calls| calls.set(calls.get() + 1));
    if INDEX_FAILS.with(std::cell::Cell::get) {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                c"index callback failure".as_ptr(),
            )
        };
        std::ptr::null_mut()
    } else {
        unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(-1) }
    }
}
unsafe extern "C" fn parse_float(_object: *mut PyObject) -> *mut PyObject {
    FLOAT_CALLS.with(|calls| calls.set(calls.get() + 1));
    unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(3.25) }
}

#[test]
fn numeric_format_units_preserve_protocol_and_target_version_admission() {
    install_hooks();
    let mut methods: Box<molt_cpython_abi::abi_types::PyNumberMethods> =
        Box::new(unsafe { std::mem::zeroed() });
    methods.nb_index = parse_index as *mut c_void;
    methods.nb_float = parse_float as *mut c_void;
    let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    ty.tp_name = c"NumericArgument".as_ptr();
    ty.tp_as_number = (&raw mut *methods).cast();
    let mut object = Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *ty,
    });
    let args = args_for_object(&raw mut *object);
    parse_numeric(&args, b'B', 77u8, u8::MAX, 1);
    parse_numeric(&args, b'H', 77u16, u16::MAX, 1);
    parse_numeric(&args, b'I', 77u32, u32::MAX, 1);
    parse_numeric(&args, b'n', 77isize, -1, 1);
    parse_numeric(&args, b'L', 77i64, -1, 1);
    for minor in [12, 13, 14] {
        NUMERIC_TARGET_MINOR.with(|value| value.set(minor));
        let before = INDEX_CALLS.with(std::cell::Cell::get);
        for code in [b'k', b'K'] {
            if minor < 14 {
                parse_numeric(&args, code, 77u64, 77, 0);
                assert!(err_is(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
                ));
                assert_eq!(
                    support::take_current_error_text().as_deref(),
                    Some("argument 1 must be int, not NumericArgument")
                );
            } else if code == b'k' {
                parse_numeric(
                    &args,
                    code,
                    77 as std::ffi::c_ulong,
                    std::ffi::c_ulong::MAX,
                    1,
                );
            } else {
                parse_numeric(&args, code, 77u64, u64::MAX, 1);
            }
        }
        assert_eq!(
            INDEX_CALLS.with(std::cell::Cell::get) - before,
            if minor < 14 { 0 } else { 2 }
        );
    }
    let before = INDEX_CALLS.with(std::cell::Cell::get);
    parse_numeric(&args, b'f', 77f32, 3.25, 1);
    parse_numeric(&args, b'd', 77f64, 3.25, 1);
    assert_eq!(FLOAT_CALLS.with(std::cell::Cell::get), 2);
    assert_eq!(INDEX_CALLS.with(std::cell::Cell::get), before);
    INDEX_FAILS.with(|value| value.set(true));
    for code in [
        b'b', b'B', b'h', b'H', b'i', b'I', b'l', b'k', b'L', b'K', b'n',
    ] {
        parse_numeric(&args, code, 77u64, 77, 0);
        assert!(err_is(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
        ));
        assert_eq!(
            support::take_current_error_text().as_deref(),
            Some("index callback failure")
        );
    }
    drop(args);
    assert_eq!(object.ob_refcnt, 1);
}

#[test]
fn format_owned_numeric_rejections_keep_argument_location_and_custom_message() {
    install_hooks();
    let number = unsafe {
        OwnedPyObject::from_owned(molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.5))
    };
    assert!(!number.as_ptr().is_null());
    let direct = args_for_object(number.as_ptr());
    let nested = args_for_object(direct.as_ptr());
    for (args, format, expected) in [
        (&direct, c"K", "argument 1 must be int, not float"),
        (
            &direct,
            c"K:consume",
            "consume() argument 1 must be int, not float",
        ),
        (
            &nested,
            c"(K):consume",
            "consume() argument 1, item 0 must be int, not float",
        ),
        (
            &nested,
            c"(K);custom conversion diagnostic",
            "custom conversion diagnostic",
        ),
    ] {
        let mut output = 77u64;
        assert_eq!(
            unsafe { PyArg_ParseTuple(args.as_ptr(), format.as_ptr(), &raw mut output) },
            0
        );
        assert_eq!(output, 77);
        assert!(err_is(
            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
        ));
        assert_eq!(
            support::take_current_error_text().as_deref(),
            Some(expected)
        );
    }
}

// ── errors.rs:512 — b/B/H store the exact C width, no adjacent clobber ──────

#[test]
fn pyarg_b_stores_one_byte_not_four() {
    install_hooks();
    clear_err();
    let args = args_with(&[0x05]);
    // Guard bytes frame the 1-byte target: a 4-byte store would zero them.
    let mut buf = [0xFFu8; 4];
    let rc = unsafe { PyArg_ParseTuple(args.as_ptr(), c"b".as_ptr(), buf.as_mut_ptr()) };
    assert_eq!(rc, 1, "'b' parse must succeed");
    assert_eq!(
        buf,
        [0x05, 0xFF, 0xFF, 0xFF],
        "'b' must store exactly ONE byte; the 3 guard bytes must survive (a \
         4-byte c_int store would zero them — the OOB-write divergence)"
    );
}

#[test]
fn pyarg_H_stores_two_bytes_not_four() {
    install_hooks();
    clear_err();
    let args = args_with(&[0x1234]);
    // A real u16 field preserves the C output's alignment on every target.
    #[repr(C)]
    struct Output {
        value: u16,
        guards: [u8; 2],
    }
    let mut out = Output {
        value: 0xFFFF,
        guards: [0xFF; 2],
    };
    let rc = unsafe { PyArg_ParseTuple(args.as_ptr(), c"H".as_ptr(), &raw mut out.value) };
    assert_eq!(rc, 1);
    assert_eq!(
        out.guards,
        [0xFF, 0xFF],
        "'H' must store exactly TWO bytes; guards past the short must survive"
    );
    assert_eq!(out.value, 0x1234, "'H' value must round-trip");
}

#[test]
fn pyarg_b_range_checks_raise_overflow() {
    install_hooks();

    clear_err();
    let args = args_with(&[256]);
    let mut out: u8 = 0;
    let rc = unsafe { PyArg_ParseTuple(args.as_ptr(), c"b".as_ptr(), &mut out as *mut u8) };
    assert_eq!(rc, 0, "'b' with 256 must fail (> UCHAR_MAX)");
    assert!(
        err_is((&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast::<PyObject>()),
        "'b' overflow must raise OverflowError"
    );

    clear_err();
    let args = args_with(&[-1]);
    let rc = unsafe { PyArg_ParseTuple(args.as_ptr(), c"b".as_ptr(), &mut out as *mut u8) };
    assert_eq!(rc, 0, "'b' with -1 must fail (< 0)");
    assert!(err_is(
        (&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError).cast::<PyObject>()
    ));
    clear_err();
}

// ── errors.rs:556 — O! reads the type, never writes through it ──────────────

#[test]
fn pyarg_o_bang_does_not_clobber_type_header_and_fills_dest() {
    install_hooks();
    clear_err();

    // A sentinel "type" whose header (ob_refcnt) the pre-fix O! grammar would
    // overwrite with the argument pointer. Its tp_base is null, so an int is NOT
    // a subtype -> the parse must FAIL, but crucially must NOT touch this header.
    let mut sentinel_type = PyTypeObject_zeroed();
    sentinel_type.ob_base.ob_base.ob_refcnt = 0x0DED_BEEF;
    sentinel_type.tp_name = c"parse.Sentinel".as_ptr();

    let args = args_with(&[7]);
    // Poison destination; must stay untouched on failure.
    let poison: *mut PyObject = std::ptr::dangling_mut::<PyObject>();
    let mut dest: *mut PyObject = poison;
    let rc = unsafe {
        PyArg_ParseTuple(
            args.as_ptr(),
            c"O!".as_ptr(),
            &raw mut sentinel_type,
            &raw mut dest,
        )
    };
    assert_eq!(
        rc, 0,
        "int is not a subtype of the sentinel type -> O! fails"
    );
    assert!(
        err_is((&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()),
        "an O! type mismatch must raise TypeError"
    );
    assert_eq!(
        sentinel_type.ob_base.ob_base.ob_refcnt, 0x0DED_BEEF,
        "O! must NOT write through the type-object pointer (header clobber = UB)"
    );
    assert_eq!(
        dest, poison,
        "a failed O! must leave the destination untouched"
    );
    clear_err();

    // Positive case: expected type == PyLong_Type, arg is an int -> stored.
    let args = args_with(&[7]);
    let refcnt_before = unsafe {
        molt_cpython_abi::abi_types::PyLong_Type
            .ob_base
            .ob_base
            .ob_refcnt
    };
    let mut dest2: *mut PyObject = std::ptr::null_mut();
    let rc = unsafe {
        PyArg_ParseTuple(
            args.as_ptr(),
            c"O!".as_ptr(),
            &raw mut molt_cpython_abi::abi_types::PyLong_Type,
            &raw mut dest2,
        )
    };
    assert_eq!(rc, 1, "an int against PyLong_Type must satisfy O!");
    assert!(
        !dest2.is_null(),
        "O! must store the object into the destination"
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_Check(dest2) },
        1,
        "the stored O! object is the int argument"
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::abi_types::PyLong_Type
                .ob_base
                .ob_base
                .ob_refcnt
        },
        refcnt_before,
        "even on success O! must not touch the type-object header"
    );
    clear_err();
}

// A zeroed PyTypeObject for the sentinel (tp_base == null => no subtypes).
fn PyTypeObject_zeroed() -> PyTypeObject {
    unsafe { std::mem::zeroed() }
}

// ── errors.rs:536 — s/z/y reject a non-string arg (no fabricated "") ────────

#[test]
fn pyarg_s_rejects_non_string_argument() {
    install_hooks();
    clear_err();
    // An int passed to 's' must be a TypeError, not a fabricated empty string
    // (the theater the pre-fix `molt_str_ptr` produced).
    let args = args_with(&[42]);
    let poison: *const c_char = std::ptr::dangling::<c_char>();
    let mut out: *const c_char = poison;
    let rc = unsafe {
        PyArg_ParseTuple(
            args.as_ptr(),
            c"s".as_ptr(),
            &mut out as *mut *const c_char as *mut c_void,
        )
    };
    assert_eq!(rc, 0, "'s' on a non-str must FAIL, not fake success");
    assert!(
        err_is((&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()),
        "'s' on a non-str must raise TypeError"
    );
    assert_eq!(
        out, poison,
        "a failed 's' must not fabricate a string pointer"
    );
    clear_err();
}

// ── errors.rs:580 — surplus positional args raise TypeError ─────────────────

#[test]
fn pyarg_surplus_positional_args_raise_typeerror() {
    install_hooks();
    clear_err();
    // format "i" consumes ONE unit; a 2-item tuple is one too many.
    let args = args_with(&[1, 2]);
    let mut out: c_int = 0;
    let rc = unsafe { PyArg_ParseTuple(args.as_ptr(), c"i".as_ptr(), &mut out as *mut c_int) };
    assert_eq!(rc, 0, "extra positional args must fail the parse");
    assert!(
        err_is((&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>()),
        "surplus args must raise TypeError (CPython 'takes at most N')"
    );
    clear_err();
}

#[test]
fn pyarg_multi_output_unit_keeps_following_output_independent() {
    install_hooks();
    clear_err();
    let args = args_with(&[0, 17]);
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::sequences::PyTuple_SetItem(
                args.as_ptr(),
                0,
                &raw mut molt_cpython_abi::abi_types::Py_None,
            )
        },
        0
    );
    // z# consumes two output addresses; i consumes the next one. This catches
    // both a reused output zero within z# and a wrong next-unit output slice.
    let mut text = std::ptr::dangling::<c_char>();
    let mut length: isize = -1;
    let mut number: c_int = -1;
    assert_eq!(
        unsafe {
            PyArg_ParseTuple(
                args.as_ptr(),
                c"z#i".as_ptr(),
                &raw mut text,
                &raw mut length,
                &raw mut number,
            )
        },
        1
    );
    assert!(text.is_null());
    assert_eq!(length, 0);
    assert_eq!(number, 17);
    assert!(unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
}

// ── errors.rs:276 — GivenExceptionMatches iterates a tuple of candidates ────
// Uses the same physical tuple authority as the parser inputs.

#[test]
fn given_exception_matches_tuple_candidates() {
    install_hooks();
    clear_err();
    // A candidate tuple (KeyError, LookupError). A pending IndexError matches
    // via the LookupError member's subclass walk; TypeError does not match.
    let candidates = [
        (&raw mut molt_cpython_abi::abi_types::PyExc_KeyError).cast::<PyObject>(),
        (&raw mut molt_cpython_abi::abi_types::PyExc_LookupError).cast::<PyObject>(),
    ];
    let tuple = unsafe {
        OwnedPyObject::from_owned(molt_cpython_abi::api::sequences::PyTuple_FromArray(
            candidates.as_ptr(),
            candidates.len() as isize,
        ))
    };
    assert!(!tuple.as_ptr().is_null());

    let hit = unsafe {
        molt_cpython_abi::api::errors::PyErr_GivenExceptionMatches(
            (&raw mut molt_cpython_abi::abi_types::PyExc_IndexError).cast::<PyObject>(),
            tuple.as_ptr(),
        )
    };
    assert_eq!(
        hit, 1,
        "except (KeyError, LookupError) must catch IndexError via the tuple \
         walk + subclass chain — the pre-fix ptr::eq never matched a tuple"
    );

    let miss = unsafe {
        molt_cpython_abi::api::errors::PyErr_GivenExceptionMatches(
            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast::<PyObject>(),
            tuple.as_ptr(),
        )
    };
    assert_eq!(miss, 0, "TypeError is in neither candidate's chain");
}

/// Error strings below are literal getargs converterr controls from the pinned
/// 3.12.15/3.13.16/3.14.8 sources, not output from the production formatter.
fn assert_converter_refusal<T: Copy + PartialEq + std::fmt::Debug>(
    args: &OwnedPyObject,
    format: &std::ffi::CStr,
    initial: T,
    expected: &str,
) {
    let mut output = NumericOutput {
        before: 0x1357_9bdf_2468_ace0,
        value: initial,
        after: 0xfedc_ba98_7654_3210,
    };
    assert_eq!(
        unsafe { PyArg_ParseTuple(args.as_ptr(), format.as_ptr(), &raw mut output.value) },
        0
    );
    assert_eq!(
        output.value, initial,
        "refusal must not write the destination"
    );
    assert_eq!(output.before, 0x1357_9bdf_2468_ace0);
    assert_eq!(output.after, 0xfedc_ba98_7654_3210);
    assert!(err_is(
        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
    ));
    assert_eq!(
        support::take_current_error_text().as_deref(),
        Some(expected)
    );
}

#[test]
fn converter_diagnostics_bound_type_bytes_and_distinguish_none_identity() {
    install_hooks();
    let mut long_type = Box::new(PyTypeObject_zeroed());
    long_type.tp_name =
        c"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_Type".as_ptr();
    let mut long_object = Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *long_type,
    });
    // A class spelling of NoneType does not confer the identity of Py_None.
    let mut named_none_type = Box::new(PyTypeObject_zeroed());
    named_none_type.tp_name = c"NoneType".as_ptr();
    let mut named_none = Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *named_none_type,
    });
    for minor in [12, 13, 14] {
        NUMERIC_TARGET_MINOR.with(|value| value.set(minor));
        for (object, expected_type) in [
            (&raw mut molt_cpython_abi::abi_types::Py_None, "None"),
            (
                &raw mut *long_object,
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwx",
            ),
            (&raw mut *named_none, "NoneType"),
        ] {
            let direct = args_for_object(object);
            let nested = args_for_object(direct.as_ptr());
            for (small, wide, args, expected) in [
                (
                    c"k",
                    c"K",
                    &direct,
                    format!("argument 1 must be int, not {expected_type}"),
                ),
                (
                    c"k:consume",
                    c"K:consume",
                    &direct,
                    format!("consume() argument 1 must be int, not {expected_type}"),
                ),
                (
                    c"(k):consume",
                    c"(K):consume",
                    &nested,
                    format!("consume() argument 1, item 0 must be int, not {expected_type}"),
                ),
                (
                    c"(k);custom conversion diagnostic",
                    c"(K);custom conversion diagnostic",
                    &nested,
                    "custom conversion diagnostic".to_string(),
                ),
            ] {
                assert_converter_refusal(args, small, 77 as std::ffi::c_ulong, &expected);
                assert_converter_refusal(args, wide, 77u64, &expected);
            }
            // O! consumes the same converter authority, including both the
            // expected type's and actual type's independent 50-byte bounds.
            let mut wanted = PyTypeObject_zeroed();
            wanted.tp_name =
                c"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ_expected".as_ptr();
            let poison = std::ptr::dangling_mut::<PyObject>();
            let mut output = poison;
            assert_eq!(
                unsafe {
                    PyArg_ParseTuple(
                        direct.as_ptr(),
                        c"O!:consume".as_ptr(),
                        &raw mut wanted,
                        &raw mut output,
                    )
                },
                0
            );
            assert_eq!(output, poison);
            assert!(err_is(
                (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast()
            ));
            let expected = format!(
                "consume() argument 1 must be abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWX, not {expected_type}"
            );
            assert_eq!(
                support::take_current_error_text().as_deref(),
                Some(expected.as_str())
            );
        }
    }
    assert_eq!(long_object.ob_refcnt, 1);
    assert_eq!(named_none.ob_refcnt, 1);
}

#[test]
fn long_typename_converter_refusal_stops_at_admission_and_preserves_callback_errors() {
    install_hooks();
    let mut methods: Box<molt_cpython_abi::abi_types::PyNumberMethods> =
        Box::new(unsafe { std::mem::zeroed() });
    methods.nb_index = parse_index as *mut c_void;
    let mut ty = Box::new(PyTypeObject_zeroed());
    ty.tp_name = c"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_Type".as_ptr();
    ty.tp_as_number = (&raw mut *methods).cast();
    let mut object = Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *ty,
    });
    let direct = args_for_object(&raw mut *object);
    let nested = args_for_object(direct.as_ptr());
    for minor in [12, 13, 14] {
        NUMERIC_TARGET_MINOR.with(|value| value.set(minor));
        INDEX_FAILS.with(|value| value.set(false));
        let before = INDEX_CALLS.with(std::cell::Cell::get);
        if minor < 14 {
            assert_converter_refusal(
                &nested,
                c"(k):consume",
                77 as std::ffi::c_ulong,
                "consume() argument 1, item 0 must be int, not ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwx",
            );
            assert_converter_refusal(
                &nested,
                c"(K);custom conversion diagnostic",
                77u64,
                "custom conversion diagnostic",
            );
            assert_eq!(INDEX_CALLS.with(std::cell::Cell::get), before);
        } else {
            parse_numeric(
                &direct,
                b'k',
                77 as std::ffi::c_ulong,
                std::ffi::c_ulong::MAX,
                1,
            );
            parse_numeric(&direct, b'K', 77u64, u64::MAX, 1);
            assert_eq!(INDEX_CALLS.with(std::cell::Cell::get), before + 2);
            INDEX_FAILS.with(|value| value.set(true));
            for format in [
                c"k:consume",
                c"K;custom conversion diagnostic",
                c"(k):consume",
                c"(K);custom conversion diagnostic",
            ] {
                let args = if format.to_bytes()[0] == b'(' {
                    &nested
                } else {
                    &direct
                };
                let mut output = NumericOutput {
                    before: 0x1357_9bdf_2468_ace0,
                    value: 77u64,
                    after: 0xfedc_ba98_7654_3210,
                };
                assert_eq!(
                    unsafe {
                        PyArg_ParseTuple(args.as_ptr(), format.as_ptr(), &raw mut output.value)
                    },
                    0
                );
                assert_eq!(output.value, 77);
                assert_eq!(output.before, 0x1357_9bdf_2468_ace0);
                assert_eq!(output.after, 0xfedc_ba98_7654_3210);
                assert!(err_is(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast()
                ));
                assert_eq!(
                    support::take_current_error_text().as_deref(),
                    Some("index callback failure")
                );
            }
            INDEX_FAILS.with(|value| value.set(false));
        }
    }
    drop(nested);
    drop(direct);
    assert_eq!(object.ob_refcnt, 1);
}
