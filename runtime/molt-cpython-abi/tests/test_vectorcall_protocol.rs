//! CALL / VECTORCALL protocol regression guards (Binary-Contract Lane 4 / D1).
//!
//! These are **mask-proof** tests: each FAILS against the pre-fix `object.rs`
//! (the PEP-590 fast path was a no-op — `PyObject_Vectorcall` returned bare NULL
//! on any `kwnames` and never read the object's vectorcall slot;
//! `PyVectorcall_Call` re-entered `PyObject_Call` and infinite-recursed when
//! used as `tp_call`) and PASSES against the CPython-3.12-faithful fix
//! (`Objects/call.c` `_PyObject_VectorcallTstate` / `_PyVectorcall_Call` /
//! `_PyObject_MakeTpCall` / `PyVectorcall_Function`).
//!
//! The harness builds throw-away vectorcall-enabled type objects + instances in
//! the test itself (never touching the runtime's type-object statics — those are
//! owned by the TYPEOBJECT lane). A test instance is a `#[repr(C)]` struct whose
//! second field is the per-object `vectorcallfunc`; its type sets
//! `Py_TPFLAGS_HAVE_VECTORCALL` and `tp_vectorcall_offset = offset_of!(…,
//! vectorcall)`, exactly the shape numpy/Cython emit for a vectorcall type.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::{
    Py_TPFLAGS_DEFAULT, Py_TPFLAGS_HAVE_VECTORCALL, Py_TPFLAGS_READY, Py_ssize_t, PyObject,
    PyTypeObject, PyVectorcallFunc,
};
use molt_cpython_abi::api::numbers::{PyLong_AsLong, PyLong_FromLong};
use molt_cpython_abi::api::object::{
    PyObject_Call, PyObject_Vectorcall, PyObject_VectorcallDict, PyVectorcall_Call,
    PyVectorcall_Function,
};
use molt_cpython_abi::api::sequences::{PyTuple_New, PyTuple_SetItem, PyTuple_Size};
use molt_cpython_abi::api::strings::PyUnicode_FromString;
use std::sync::{Mutex, Once};

// Serialize these tests: they share the `REC`/`TPREC` recording statics, and the
// default cargo harness runs a binary's tests on parallel threads.
static TEST_LOCK: Mutex<()> = Mutex::new(());
static INIT: Once = Once::new();

const OFFSET_BIT: usize = 1usize << (8 * std::mem::size_of::<usize>() - 1);
const SENTINEL_VC: std::os::raw::c_long = 24_601; // returned by the vectorcall slot
const SENTINEL_TP: std::os::raw::c_long = 90_210; // returned by the tp_call recorder

fn nargs_of(nargsf: usize) -> isize {
    (nargsf & !OFFSET_BIT) as isize
}

// ── recording of what the vectorcall slot received ──────────────────────────
#[derive(Clone, Copy)]
struct Rec {
    calls: u32,
    nargs: isize,
    saw_offset_bit: bool,
    kwnames_addr: usize,
    kwnames_size: isize,
    first_arg_addr: usize,
}
const REC_EMPTY: Rec = Rec {
    calls: 0,
    nargs: -1,
    saw_offset_bit: false,
    kwnames_addr: 0,
    kwnames_size: -1,
    first_arg_addr: 0,
};
static REC: Mutex<Rec> = Mutex::new(REC_EMPTY);

// ── recording of what a tp_call slot received (the fallback / anti-slot proof) ─
#[derive(Clone, Copy)]
struct TpRec {
    calls: u32,
    args_size: isize,
    kwargs_addr: usize,
}
const TPREC_EMPTY: TpRec = TpRec {
    calls: 0,
    args_size: -2,
    kwargs_addr: 0,
};
static TPREC: Mutex<TpRec> = Mutex::new(TPREC_EMPTY);

/// A vectorcall-enabled instance: `vectorcall` MUST be the field at
/// `tp_vectorcall_offset` so `PyVectorcall_Function` can read it.
#[repr(C)]
struct VcInstance {
    ob_base: PyObject,
    vectorcall: Option<PyVectorcallFunc>,
}

fn setup() {
    INIT.call_once(|| {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        // A str-materializing hook so `PyUnicode_FromString` (kwnames keys) works;
        // inline ints (`PyLong_FromLong`) and ABI-layout tuples need no hooks.
        let mut hooks = molt_cpython_abi::hooks::STUB_HOOKS;
        hooks.alloc_str = fake_alloc_str;
        hooks.str_data = fake_str_data;
        unsafe {
            let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
        }
    });
    *REC.lock().unwrap() = REC_EMPTY;
    *TPREC.lock().unwrap() = TPREC_EMPTY;
}

// Minimal leaked-bytes str arena (mirrors frontier_repro.rs).
static STR_ARENA: Mutex<Option<std::collections::HashMap<u64, &'static [u8]>>> = Mutex::new(None);
static NEXT_STR_HANDLE: Mutex<u64> = Mutex::new(0x4200_0000);

unsafe extern "C" fn fake_alloc_str(data: *const u8, len: usize) -> u64 {
    let payload: &[u8] = if data.is_null() || len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    let mut owned = payload.to_vec();
    owned.push(0);
    let leaked: &'static [u8] = Box::leak(owned.into_boxed_slice());
    let mut next = NEXT_STR_HANDLE.lock().unwrap();
    let handle = *next;
    *next += 0x10;
    drop(next);
    STR_ARENA
        .lock()
        .unwrap()
        .get_or_insert_with(std::collections::HashMap::new)
        .insert(handle, leaked);
    handle
}

unsafe extern "C" fn fake_str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let arena = STR_ARENA.lock().unwrap();
    if let Some(bytes) = arena.as_ref().and_then(|m| m.get(&bits)) {
        if !out_len.is_null() {
            unsafe { *out_len = bytes.len().saturating_sub(1) };
        }
        bytes.as_ptr()
    } else {
        if !out_len.is_null() {
            unsafe { *out_len = 0 };
        }
        std::ptr::null()
    }
}

/// The recording vectorcall slot: logs `nargs`/kwnames/offset-bit and returns a
/// known sentinel int so callers can prove the slot (not a fallback) ran.
unsafe extern "C" fn rec_vectorcall(
    _callable: *mut PyObject,
    args: *mut *mut PyObject,
    nargsf: usize,
    kwnames: *mut PyObject,
) -> *mut PyObject {
    let mut r = REC.lock().unwrap();
    r.calls += 1;
    r.nargs = nargs_of(nargsf);
    r.saw_offset_bit = (nargsf & OFFSET_BIT) != 0;
    r.kwnames_addr = kwnames as usize;
    r.kwnames_size = if kwnames.is_null() {
        -1
    } else {
        unsafe { PyTuple_Size(kwnames) }
    };
    r.first_arg_addr = if args.is_null() {
        0
    } else {
        (unsafe { *args }) as usize
    };
    drop(r);
    unsafe { PyLong_FromLong(SENTINEL_VC) }
}

/// A tp_call recorder: proves whether the slow tp_call path was taken (it must
/// NOT be, when a vectorcall slot exists) and, in the fallback tests, that the
/// materialized args tuple has the right size.
unsafe extern "C" fn rec_tpcall(
    _self: *mut PyObject,
    args: *mut PyObject,
    kwargs: *mut PyObject,
) -> *mut PyObject {
    let mut r = TPREC.lock().unwrap();
    r.calls += 1;
    r.args_size = if args.is_null() {
        -1
    } else {
        unsafe { PyTuple_Size(args) }
    };
    r.kwargs_addr = kwargs as usize;
    drop(r);
    unsafe { PyLong_FromLong(SENTINEL_TP) }
}

type TpCall = unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> *mut PyObject;

/// Leak a type object carrying the given flags / vectorcall-offset / tp_call.
fn make_type(
    flags: std::os::raw::c_ulong,
    vc_offset: Py_ssize_t,
    tp_call: Option<TpCall>,
) -> *mut PyTypeObject {
    let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
    ty.ob_base.ob_base.ob_refcnt = 1 << 30; // immortal-ish; never deallocate
    ty.tp_name = c"molt.test.Vc".as_ptr();
    ty.tp_flags = flags;
    ty.tp_vectorcall_offset = vc_offset;
    ty.tp_call = tp_call;
    Box::leak(Box::new(ty)) as *mut PyTypeObject
}

/// Build a leaked vectorcall instance whose per-object slot is `slot`. The type
/// always carries a valid `tp_vectorcall_offset` (so `PyVectorcall_Call` can read
/// the slot directly), but `Py_TPFLAGS_HAVE_VECTORCALL` — which gates the
/// `PyObject_Call`/`PyObject_Vectorcall` vectorcall-first probe — is set only
/// when `have_vectorcall` is true. Passing `false` forces `PyObject_Call` to
/// reach `tp_call` (the exact pre-fix recursion scenario).
fn make_vc_instance(
    have_vectorcall: bool,
    slot: Option<PyVectorcallFunc>,
    tp_call: Option<TpCall>,
) -> *mut PyObject {
    let vc_offset = std::mem::offset_of!(VcInstance, vectorcall) as Py_ssize_t;
    let mut flags = Py_TPFLAGS_READY | Py_TPFLAGS_DEFAULT;
    if have_vectorcall {
        flags |= Py_TPFLAGS_HAVE_VECTORCALL;
    }
    let ty = make_type(flags, vc_offset, tp_call);
    let inst = VcInstance {
        ob_base: PyObject {
            ob_refcnt: 1 << 30,
            ob_type: ty,
        },
        vectorcall: slot,
    };
    Box::leak(Box::new(inst)) as *mut VcInstance as *mut PyObject
}

fn read_long(op: *mut PyObject) -> std::os::raw::c_long {
    unsafe { PyLong_AsLong(op) }
}

// ════════════════════════════════════════════════════════════════════════════
// #7 — PyObject_Vectorcall reads the slot AND forwards kwnames.
// Pre-fix: `if !kwnames.is_null() { return NULL }` → bare NULL, slot never run.
// ════════════════════════════════════════════════════════════════════════════
#[test]
fn vectorcall_reads_slot_and_forwards_kwnames() {
    let _g = TEST_LOCK.lock().unwrap();
    setup();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    // tp_call = None: any degrade-to-tuple-then-tp_call path would find no slot
    // and could not produce SENTINEL_VC — so a correct SENTINEL_VC proves the
    // object's OWN vectorcallfunc ran.
    let inst = make_vc_instance(true, Some(rec_vectorcall), None);

    // 2 positional + 1 keyword value; kwnames = ("kw",).
    let a0 = unsafe { PyLong_FromLong(10) };
    let a1 = unsafe { PyLong_FromLong(20) };
    let kwval = unsafe { PyLong_FromLong(30) };
    let mut argv: [*mut PyObject; 3] = [a0, a1, kwval];
    let kwnames = unsafe { PyTuple_New(1) };
    let kwkey = unsafe { PyUnicode_FromString(c"kw".as_ptr()) };
    assert!(!kwnames.is_null() && !kwkey.is_null());
    unsafe {
        PyTuple_SetItem(kwnames, 0, kwkey);
    }

    let result = unsafe { PyObject_Vectorcall(inst, argv.as_mut_ptr(), 2, kwnames) };
    assert!(
        !result.is_null(),
        "PyObject_Vectorcall returned NULL — the pre-fix kwnames no-op; the slot \
         was never consulted (SystemError-without-error frontier)"
    );
    assert_eq!(
        read_long(result),
        SENTINEL_VC,
        "the object's vectorcall slot must have run"
    );

    let r = *REC.lock().unwrap();
    assert_eq!(r.calls, 1, "slot must be invoked exactly once");
    assert_eq!(
        r.nargs, 2,
        "PyVectorcall_NARGS(nargsf) must be 2 positional"
    );
    assert_eq!(
        r.kwnames_size, 1,
        "the kwnames tuple must be forwarded intact"
    );
    assert_eq!(
        r.kwnames_addr, kwnames as usize,
        "the exact kwnames pointer is forwarded"
    );
    assert_eq!(
        r.first_arg_addr, a0 as usize,
        "the argument array is forwarded verbatim"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// #7 — the slot is used even without keywords (no silent degrade to tp_call).
// Pre-fix: PyObject_Vectorcall built a tuple and called PyObject_Call → tp_call,
// bypassing the object's vectorcallfunc entirely.
// ════════════════════════════════════════════════════════════════════════════
#[test]
fn vectorcall_uses_slot_not_tp_call() {
    let _g = TEST_LOCK.lock().unwrap();
    setup();

    // Both a real slot AND a tp_call recorder: a correct impl runs the slot
    // (SENTINEL_VC, TPREC.calls == 0); the pre-fix impl runs tp_call (SENTINEL_TP).
    let inst = make_vc_instance(true, Some(rec_vectorcall), Some(rec_tpcall));
    let a0 = unsafe { PyLong_FromLong(7) };
    let mut argv: [*mut PyObject; 1] = [a0];

    let result = unsafe { PyObject_Vectorcall(inst, argv.as_mut_ptr(), 1, std::ptr::null_mut()) };
    assert_eq!(
        read_long(result),
        SENTINEL_VC,
        "the vectorcall slot must run, not tp_call"
    );
    assert_eq!(REC.lock().unwrap().calls, 1);
    assert_eq!(
        TPREC.lock().unwrap().calls,
        0,
        "tp_call must NOT be reached when a vectorcall slot exists"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// #6 — tp_call = PyVectorcall_Call TERMINATES (no infinite recursion).
// Pre-fix: PyObject_Call → tp_call(=PyVectorcall_Call) → PyObject_Call → …
// stack overflow / SIGSEGV. The documented, intended pattern for vectorcall types.
// ════════════════════════════════════════════════════════════════════════════
#[test]
fn tp_call_is_pyvectorcall_call_terminates() {
    let _g = TEST_LOCK.lock().unwrap();
    setup();

    // A real positional args tuple, as PyObject_Call requires.
    let args = unsafe { PyTuple_New(2) };
    unsafe {
        PyTuple_SetItem(args, 0, PyLong_FromLong(1));
        PyTuple_SetItem(args, 1, PyLong_FromLong(2));
    }

    // Scenario 1 — the EXACT pre-fix recursion path: NO HAVE_VECTORCALL flag, so
    // PyObject_Call's vectorcall-first probe misses and it dispatches through
    // tp_call = PyVectorcall_Call. Pre-fix that re-entered PyObject_Call → tp_call
    // → … (stack overflow). Post-fix PyVectorcall_Call reads tp_vectorcall_offset
    // directly and calls the slot — terminating with the correct result.
    let via_tp_call = make_vc_instance(false, Some(rec_vectorcall), Some(PyVectorcall_Call));
    let calls0 = REC.lock().unwrap().calls;
    let r1 = unsafe { PyObject_Call(via_tp_call, args, std::ptr::null_mut()) };
    assert_eq!(
        read_long(r1),
        SENTINEL_VC,
        "PyObject_Call → tp_call=PyVectorcall_Call must terminate at the slot, not recurse"
    );
    assert_eq!(
        REC.lock().unwrap().calls,
        calls0 + 1,
        "exactly one slot invocation — no recursive re-entry"
    );

    // Scenario 2 — with the flag set, PyObject_Call goes vectorcall-first and
    // still terminates at the slot (never touching tp_call).
    let via_flag = make_vc_instance(true, Some(rec_vectorcall), Some(PyVectorcall_Call));
    let r2 = unsafe { PyObject_Call(via_flag, args, std::ptr::null_mut()) };
    assert_eq!(
        read_long(r2),
        SENTINEL_VC,
        "vectorcall-first path must also reach the slot"
    );

    // Scenario 3 — PyVectorcall_Call invoked directly reads the slot, no recursion.
    let calls1 = REC.lock().unwrap().calls;
    let r3 = unsafe { PyVectorcall_Call(via_flag, args, std::ptr::null_mut()) };
    assert_eq!(
        read_long(r3),
        SENTINEL_VC,
        "direct PyVectorcall_Call must invoke the slot"
    );
    assert_eq!(
        REC.lock().unwrap().calls,
        calls1 + 1,
        "one further slot call, no recursion"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// #6 — PyVectorcall_Call on an object with NO vectorcall slot raises TypeError
// (never recurses, never bare-NULLs). Mirrors CPython "'…' object does not
// support vectorcall".
// ════════════════════════════════════════════════════════════════════════════
#[test]
fn pyvectorcall_call_without_slot_raises_typeerror() {
    let _g = TEST_LOCK.lock().unwrap();
    setup();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    // No HAVE_VECTORCALL flag, offset 0, but tp_call set — PyVectorcall_Call must
    // still refuse (it reads the offset directly and finds none), not recurse.
    let ty = make_type(Py_TPFLAGS_READY | Py_TPFLAGS_DEFAULT, 0, Some(rec_tpcall));
    let inst = Box::leak(Box::new(PyObject {
        ob_refcnt: 1 << 30,
        ob_type: ty,
    })) as *mut PyObject;
    let args = unsafe { PyTuple_New(0) };

    let result = unsafe { PyVectorcall_Call(inst, args, std::ptr::null_mut()) };
    assert!(
        result.is_null(),
        "no-vectorcall object must not return a value"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a NULL PyVectorcall_Call must set TypeError, never a bare NULL"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// ════════════════════════════════════════════════════════════════════════════
// PyVectorcall_Function accessor (previously MISSING) — reads the slot via
// HAVE_VECTORCALL + tp_vectorcall_offset; NULL for non-vectorcall objects.
// ════════════════════════════════════════════════════════════════════════════
#[test]
fn pyvectorcall_function_reads_slot() {
    let _g = TEST_LOCK.lock().unwrap();
    setup();

    let inst = make_vc_instance(true, Some(rec_vectorcall), None);
    let got = unsafe { PyVectorcall_Function(inst) };
    assert!(
        got.is_some(),
        "PyVectorcall_Function must return the object's slot"
    );
    assert_eq!(
        got.unwrap() as *const (),
        rec_vectorcall as *const (),
        "the returned vectorcallfunc must be the object's own slot"
    );

    // A plain object (no HAVE_VECTORCALL flag) → None.
    let plain_ty = make_type(Py_TPFLAGS_READY | Py_TPFLAGS_DEFAULT, 0, None);
    let plain = Box::leak(Box::new(PyObject {
        ob_refcnt: 1 << 30,
        ob_type: plain_ty,
    })) as *mut PyObject;
    assert!(
        unsafe { PyVectorcall_Function(plain) }.is_none(),
        "a non-vectorcall object has no vectorcallfunc"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// PyObject_Vectorcall forwards nargsf UNMASKED; PY_VECTORCALL_ARGUMENTS_OFFSET
// survives to the slot, and PyVectorcall_NARGS masks it back to the true count.
// ════════════════════════════════════════════════════════════════════════════
#[test]
fn vectorcall_forwards_arguments_offset_bit() {
    let _g = TEST_LOCK.lock().unwrap();
    setup();

    let inst = make_vc_instance(true, Some(rec_vectorcall), None);
    let a0 = unsafe { PyLong_FromLong(5) };
    let a1 = unsafe { PyLong_FromLong(6) };
    let mut argv: [*mut PyObject; 2] = [a0, a1];

    let nargsf = 2usize | OFFSET_BIT;
    let result =
        unsafe { PyObject_Vectorcall(inst, argv.as_mut_ptr(), nargsf, std::ptr::null_mut()) };
    assert_eq!(read_long(result), SENTINEL_VC);
    let r = *REC.lock().unwrap();
    assert_eq!(
        r.nargs, 2,
        "PyVectorcall_NARGS must mask the offset bit → 2"
    );
    assert!(
        r.saw_offset_bit,
        "the ARGUMENTS_OFFSET bit must reach the slot unmasked"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// PyObject_VectorcallDict masks PY_VECTORCALL_ARGUMENTS_OFFSET before building
// the fallback args tuple.  Pre-fix: `nargs as isize` leaked the high bit → a
// negative count → NULL. Uses the tp_call fallback (no dict hooks needed).
// ════════════════════════════════════════════════════════════════════════════
#[test]
fn vectorcall_dict_masks_nargs() {
    let _g = TEST_LOCK.lock().unwrap();
    setup();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };

    // Non-vectorcall object with a tp_call recorder → VectorcallDict takes the
    // MakeTpCall fallback and must build a correctly-sized (2) positional tuple.
    let ty = make_type(Py_TPFLAGS_READY | Py_TPFLAGS_DEFAULT, 0, Some(rec_tpcall));
    let inst = Box::leak(Box::new(PyObject {
        ob_refcnt: 1 << 30,
        ob_type: ty,
    })) as *mut PyObject;

    let a0 = unsafe { PyLong_FromLong(100) };
    let a1 = unsafe { PyLong_FromLong(200) };
    let mut argv: [*mut PyObject; 2] = [a0, a1];

    let nargsf = 2usize | OFFSET_BIT; // caller set the offset bit
    let result =
        unsafe { PyObject_VectorcallDict(inst, argv.as_mut_ptr(), nargsf, std::ptr::null_mut()) };
    assert_eq!(
        read_long(result),
        SENTINEL_TP,
        "VectorcallDict must mask the offset bit and reach tp_call (pre-fix returned NULL)"
    );
    assert_eq!(
        TPREC.lock().unwrap().args_size,
        2,
        "the materialized positional tuple must hold exactly 2 args, not a bit-leaked count"
    );
}

// ════════════════════════════════════════════════════════════════════════════
// No-vectorcall-slot fallback: PyObject_Vectorcall builds a positional tuple and
// dispatches through tp_call (CPython _PyObject_MakeTpCall). tuple-only, no dict.
// ════════════════════════════════════════════════════════════════════════════
#[test]
fn vectorcall_falls_back_to_tp_call_without_slot() {
    let _g = TEST_LOCK.lock().unwrap();
    setup();

    let ty = make_type(Py_TPFLAGS_READY | Py_TPFLAGS_DEFAULT, 0, Some(rec_tpcall));
    let inst = Box::leak(Box::new(PyObject {
        ob_refcnt: 1 << 30,
        ob_type: ty,
    })) as *mut PyObject;

    let a0 = unsafe { PyLong_FromLong(1) };
    let a1 = unsafe { PyLong_FromLong(2) };
    let a2 = unsafe { PyLong_FromLong(3) };
    let mut argv: [*mut PyObject; 3] = [a0, a1, a2];

    let result = unsafe { PyObject_Vectorcall(inst, argv.as_mut_ptr(), 3, std::ptr::null_mut()) };
    assert_eq!(
        read_long(result),
        SENTINEL_TP,
        "fallback must dispatch through tp_call"
    );
    assert_eq!(
        TPREC.lock().unwrap().args_size,
        3,
        "fallback must materialize all 3 positional args into the tp_call tuple"
    );
}
