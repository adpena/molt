//! F1 mask-proof gates for the `PyArg_ParseTuple` format engine
//! (`molt_pyarg_parse_tuple_inner` + the `pyarg_variadic.c` shim).
//!
//! These install a process-global mock hook table that backs a single synthetic
//! args tuple with a caller-controlled `Vec` of item handles, then drive the
//! REAL variadic `PyArg_ParseTuple` (so the shim's `count_format_outs` vararg
//! accounting is exercised end-to-end, not just the Rust inner).
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

use molt_cpython_abi::abi_types::{PyObject, PyTypeObject};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::MoltObject;
use std::ffi::{c_char, c_int, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

// The bits of the one synthetic tuple the mock hooks answer for.
static TUPLE_BITS: AtomicU64 = AtomicU64::new(0);
// The item handles of that tuple (guarded by TEST_LOCK while a test runs).
static ITEMS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
// Serializes tests: the mock table + ITEMS + TUPLE_BITS are process-global.
static TEST_LOCK: Mutex<()> = Mutex::new(());

unsafe extern "C" fn mock_tuple_len(bits: u64) -> usize {
    if bits == TUPLE_BITS.load(Ordering::SeqCst) {
        ITEMS.lock().unwrap().len()
    } else {
        0
    }
}

unsafe extern "C" fn mock_tuple_item(bits: u64, i: usize) -> u64 {
    if bits == TUPLE_BITS.load(Ordering::SeqCst) {
        ITEMS.lock().unwrap().get(i).copied().unwrap_or(0)
    } else {
        0
    }
}

unsafe extern "C" fn mock_classify_heap(bits: u64) -> u8 {
    if bits == TUPLE_BITS.load(Ordering::SeqCst) {
        molt_cpython_abi::abi_types::MoltTypeTag::Tuple as u8
    } else {
        molt_cpython_abi::abi_types::MoltTypeTag::Other as u8
    }
}

fn install_hooks() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    // Allocate a stable backing pointer for the synthetic tuple handle exactly
    // once; register its proxy so `pyobj_to_handle(args)` resolves to the bits
    // the mock tuple hooks answer for.
    if TUPLE_BITS.load(Ordering::SeqCst) == 0 {
        let backing: *mut u8 = Box::into_raw(Box::new(0u8));
        let bits = MoltObject::from_ptr(backing).bits();
        TUPLE_BITS.store(bits, Ordering::SeqCst);
    }
    let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.tuple_len = mock_tuple_len;
    hooks.tuple_item = mock_tuple_item;
    hooks.classify_heap = mock_classify_heap;
    unsafe {
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

/// Set the synthetic tuple's items and return its args `PyObject*`.
fn args_with(items: &[u64]) -> *mut PyObject {
    *ITEMS.lock().unwrap() = items.to_vec();
    let bits = TUPLE_BITS.load(Ordering::SeqCst);
    unsafe { GLOBAL_BRIDGE.lock().handle_to_pyobj(bits) }
}

fn int_item(v: i64) -> u64 {
    MoltObject::from_int(v).bits()
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
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let args = args_with(&[int_item(0x05)]);
    // Guard bytes frame the 1-byte target: a 4-byte store would zero them.
    let mut buf = [0xFFu8; 4];
    let rc = unsafe { PyArg_ParseTuple(args, c"b".as_ptr(), buf.as_mut_ptr()) };
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
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    let args = args_with(&[int_item(0x1234)]);
    let mut buf = [0xFFu8; 4];
    // 'H' target is an unsigned short (2 bytes); pass a u16* worth of storage.
    let rc = unsafe { PyArg_ParseTuple(args, c"H".as_ptr(), buf.as_mut_ptr().cast::<u16>()) };
    assert_eq!(rc, 1);
    // little-endian 0x1234 -> [0x34,0x12]; guards [2],[3] must survive.
    assert_eq!(
        [buf[2], buf[3]],
        [0xFF, 0xFF],
        "'H' must store exactly TWO bytes; guards past the short must survive"
    );
    assert_eq!([buf[0], buf[1]], [0x34, 0x12], "'H' value must round-trip");
}

#[test]
fn pyarg_b_range_checks_raise_overflow() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();

    clear_err();
    let args = args_with(&[int_item(256)]);
    let mut out: u8 = 0;
    let rc = unsafe { PyArg_ParseTuple(args, c"b".as_ptr(), &mut out as *mut u8) };
    assert_eq!(rc, 0, "'b' with 256 must fail (> UCHAR_MAX)");
    assert!(
        err_is(&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError),
        "'b' overflow must raise OverflowError"
    );

    clear_err();
    let args = args_with(&[int_item(-1)]);
    let rc = unsafe { PyArg_ParseTuple(args, c"b".as_ptr(), &mut out as *mut u8) };
    assert_eq!(rc, 0, "'b' with -1 must fail (< 0)");
    assert!(err_is(&raw mut molt_cpython_abi::abi_types::PyExc_OverflowError));
    clear_err();
}

// ── errors.rs:556 — O! reads the type, never writes through it ──────────────

#[test]
fn pyarg_o_bang_does_not_clobber_type_header_and_fills_dest() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();

    // A sentinel "type" whose header (ob_refcnt) the pre-fix O! grammar would
    // overwrite with the argument pointer. Its tp_base is null, so an int is NOT
    // a subtype -> the parse must FAIL, but crucially must NOT touch this header.
    let mut sentinel_type = PyTypeObject_zeroed();
    sentinel_type.ob_base.ob_base.ob_refcnt = 0x0DED_BEEF;

    let args = args_with(&[int_item(7)]);
    // Poison destination; must stay untouched on failure.
    let poison: *mut PyObject = std::ptr::dangling_mut::<PyObject>();
    let mut dest: *mut PyObject = poison;
    let rc = unsafe {
        PyArg_ParseTuple(
            args,
            c"O!".as_ptr(),
            &raw mut sentinel_type,
            &raw mut dest,
        )
    };
    assert_eq!(rc, 0, "int is not a subtype of the sentinel type -> O! fails");
    assert!(
        err_is(&raw mut molt_cpython_abi::abi_types::PyExc_TypeError),
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
    let args = args_with(&[int_item(7)]);
    let refcnt_before =
        unsafe { molt_cpython_abi::abi_types::PyLong_Type.ob_base.ob_base.ob_refcnt };
    let mut dest2: *mut PyObject = std::ptr::null_mut();
    let rc = unsafe {
        PyArg_ParseTuple(
            args,
            c"O!".as_ptr(),
            &raw mut molt_cpython_abi::abi_types::PyLong_Type,
            &raw mut dest2,
        )
    };
    assert_eq!(rc, 1, "an int against PyLong_Type must satisfy O!");
    assert!(!dest2.is_null(), "O! must store the object into the destination");
    assert_eq!(
        unsafe { molt_cpython_abi::api::numbers::PyLong_Check(dest2) },
        1,
        "the stored O! object is the int argument"
    );
    assert_eq!(
        unsafe { molt_cpython_abi::abi_types::PyLong_Type.ob_base.ob_base.ob_refcnt },
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
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    // An int passed to 's' must be a TypeError, not a fabricated empty string
    // (the theater the pre-fix `molt_str_ptr` produced).
    let args = args_with(&[int_item(42)]);
    let poison: *const c_char = std::ptr::dangling::<c_char>();
    let mut out: *const c_char = poison;
    let rc = unsafe {
        PyArg_ParseTuple(args, c"s".as_ptr(), &mut out as *mut *const c_char as *mut c_void)
    };
    assert_eq!(rc, 0, "'s' on a non-str must FAIL, not fake success");
    assert!(
        err_is(&raw mut molt_cpython_abi::abi_types::PyExc_TypeError),
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
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    // format "i" consumes ONE unit; a 2-item tuple is one too many.
    let args = args_with(&[int_item(1), int_item(2)]);
    let mut out: c_int = 0;
    let rc = unsafe { PyArg_ParseTuple(args, c"i".as_ptr(), &mut out as *mut c_int) };
    assert_eq!(rc, 0, "extra positional args must fail the parse");
    assert!(
        err_is(&raw mut molt_cpython_abi::abi_types::PyExc_TypeError),
        "surplus args must raise TypeError (CPython 'takes at most N')"
    );
    clear_err();
}

// ── errors.rs:276 — GivenExceptionMatches iterates a tuple of candidates ────
// (Lives in this binary because its mock hook table backs a synthetic tuple.)

#[test]
fn given_exception_matches_tuple_candidates() {
    let _g = TEST_LOCK.lock().unwrap();
    install_hooks();
    clear_err();
    // A candidate tuple (KeyError, LookupError). A pending IndexError matches
    // via the LookupError member's subclass walk; TypeError does not match.
    let key_bits = GLOBAL_BRIDGE
        .lock()
        .pyobj_to_handle(&raw mut molt_cpython_abi::abi_types::PyExc_KeyError)
        .map(|identity| identity.as_handle())
        .expect("exception singletons are bridge-registered");
    let lookup_bits = GLOBAL_BRIDGE
        .lock()
        .pyobj_to_handle(&raw mut molt_cpython_abi::abi_types::PyExc_LookupError)
        .map(|identity| identity.as_handle())
        .expect("exception singletons are bridge-registered");
    let tuple = args_with(&[key_bits, lookup_bits]); // the mock tuple object

    let hit = unsafe {
        molt_cpython_abi::api::errors::PyErr_GivenExceptionMatches(
            &raw mut molt_cpython_abi::abi_types::PyExc_IndexError,
            tuple,
        )
    };
    assert_eq!(
        hit, 1,
        "except (KeyError, LookupError) must catch IndexError via the tuple \
         walk + subclass chain — the pre-fix ptr::eq never matched a tuple"
    );

    let miss = unsafe {
        molt_cpython_abi::api::errors::PyErr_GivenExceptionMatches(
            &raw mut molt_cpython_abi::abi_types::PyExc_TypeError,
            tuple,
        )
    };
    assert_eq!(miss, 0, "TypeError is in neither candidate's chain");
}
