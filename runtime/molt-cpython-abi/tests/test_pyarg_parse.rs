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
    support::prepare_abi_test_thread(support::stub_runtime_hooks());
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
