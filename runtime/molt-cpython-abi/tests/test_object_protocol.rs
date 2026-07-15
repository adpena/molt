//! Tests for object protocol: PyObject_Repr, Str, Hash, RichCompare,
//! TypeCheck, IsInstance, CallableCheck.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::Py_NotImplementedSentinel;
use molt_cpython_abi::abi_types::{
    METH_NOARGS, METH_O, Py_OptimizeFlag, Py_buffer, Py_ssize_t, PyBUF_FORMAT, PyBUF_READ,
    PyBUF_STRIDES, PyBUF_WRITE, PyMethodDef, PyMutex, PyObject, is_immortal_refcnt,
};
use std::ffi::c_void;
use std::os::raw::c_char;
use std::ptr;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Serializes EVERY test in this binary. Many of these tests (e.g.
/// `test_richcompare_native_int_ordering_is_computed`) create small-int operands
/// via `PyLong_FromLong` and depend on `PyObject_RichCompare` / `PyObject_Hash`
/// resolving them back through the process-global `GLOBAL_BRIDGE` small-int proxy
/// cache. Sibling tests that create the SAME value share ONE deduped, mortal
/// proxy whose `ob_refcnt` the ABI mutates WITHOUT the bridge lock
/// (`Py_INCREF`/`Py_DECREF`); under `cargo test`'s parallel threads those
/// non-atomic refcount RMWs race, evicting a still-live handle so a comparison
/// yields NotImplemented instead of the computed result. Production serializes
/// every C-extension ABI call via the GIL; this lock restores that invariant for
/// the harness (the `TEST_LOCK` convention used across this crate's tests). It is
/// pure isolation — serial `--test-threads=1` was already 0-flake and every
/// assertion is unchanged.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the binary-wide serialization guard (poison-tolerant) and run the
/// idempotent ABI init. Every test binds the returned `MutexGuard` for its body.
fn init() -> MutexGuard<'static, ()> {
    let guard = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    guard
}

/// Expected `ob_refcnt` after ONE new-reference `Py_INCREF`. CPython-faithful:
/// an IMMORTAL object (builtin type statics like `PyLong_Type` are immortal on
/// 3.12+) never bumps, so the count is UNCHANGED; a mortal object shows
/// `before + 1`. Routes through the crate's single `is_immortal_refcnt`
/// authority so the mortal case is NOT weakened.
fn refcnt_after_one_incref(before: Py_ssize_t) -> Py_ssize_t {
    if is_immortal_refcnt(before) {
        before
    } else {
        before + 1
    }
}

// ---------------------------------------------------------------------------
// PyObject_Repr / PyObject_Str
// ---------------------------------------------------------------------------

#[test]
fn test_object_repr_fails_closed_under_stubs() {
    // PyObject_Repr builds its result string via PyUnicode_FromString, whose
    // alloc_str fails under the stub table => NULL + MemoryError. Post-burndown
    // that path fails closed instead of returning a fabricated None placeholder.
    let _guard = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let repr = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(py) };
    assert!(
        repr.is_null(),
        "PyObject_Repr string alloc fails closed under stubs"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_object_repr_null_returns_null() {
    let _guard = init();
    let repr = unsafe { molt_cpython_abi::api::typeobj::PyObject_Repr(ptr::null_mut()) };
    assert!(repr.is_null());
}

#[test]
fn test_object_str_fails_closed_under_stubs() {
    // Same as repr: PyObject_Str's result-string alloc fails closed under stubs.
    let _guard = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let s = unsafe { molt_cpython_abi::api::typeobj::PyObject_Str(py) };
    assert!(
        s.is_null(),
        "PyObject_Str string alloc fails closed under stubs"
    );
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_memoryview_from_memory_has_type_and_null_base() {
    let _guard = init();
    let mut byte = b'x' as c_char;
    let view = unsafe {
        molt_cpython_abi::api::memory::PyMemoryView_FromMemory(&mut byte, 1, PyBUF_WRITE)
    };
    assert!(!view.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::memory::PyMemoryView_Check(view) },
        1
    );
    assert!(unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BASE(view) }.is_null());
    let buffer = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(view) };
    assert!(!buffer.is_null());
    assert_eq!(unsafe { (*buffer).len }, 1);
    // FromMemory fills the embedded view IN PLACE via the CPython-exact
    // (allocation-free) FillInfo: internal is NULL and shape/strides are the
    // self-referential field pointers, which live inside the memoryview
    // object itself and stay valid for its whole lifetime.
    assert!(unsafe { (*buffer).internal }.is_null());
    assert!(!unsafe { (*buffer).format }.is_null());
    assert!(!unsafe { (*buffer).shape }.is_null());
    assert!(!unsafe { (*buffer).strides }.is_null());
    assert!(
        std::ptr::eq(unsafe { (*buffer).shape }.cast_const(), unsafe {
            &raw const (*buffer).len
        },),
        "FromMemory shape must self-point at the embedded view's len",
    );
    unsafe {
        assert_eq!(*(*buffer).format as u8, b'B');
        assert_eq!(*(*buffer).shape, 1);
        assert_eq!(*(*buffer).strides, 1);
    }
    // CPython: FromObject(memoryview) returns a NEW distinct memoryview sharing
    // the source's buffer (mbuf_add_view) — never the same object aliased.
    let second_view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromObject(view) };
    assert_ne!(
        second_view, view,
        "FromObject(mv) must mint a distinct view"
    );
    let second_buffer =
        unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(second_view) };
    assert_eq!(
        unsafe { (*second_buffer).buf },
        unsafe { (*buffer).buf },
        "the new view shares the source's memory"
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(second_view) };
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };

    let empty_view = unsafe {
        molt_cpython_abi::api::memory::PyMemoryView_FromMemory(ptr::null_mut(), 0, PyBUF_READ)
    };
    assert!(!empty_view.is_null());
    let empty_buffer =
        unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(empty_view) };
    assert!(!empty_buffer.is_null());
    assert_eq!(unsafe { (*empty_buffer).len }, 0);
    assert!(unsafe { (*empty_buffer).buf }.is_null());
    assert!(unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BASE(empty_view) }.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(empty_view) };
}

#[test]
fn test_memoryview_from_buffer_copies_descriptor_without_sharing_release() {
    let _guard = init();
    let mut bytes = [1_u8, 2, 3, 4];
    let mut info: Py_buffer = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        molt_cpython_abi::api::buffer::PyBuffer_FillInfo(
            &mut info,
            ptr::null_mut(),
            bytes.as_mut_ptr().cast(),
            bytes.len() as isize,
            1,
            PyBUF_FORMAT | PyBUF_STRIDES,
        )
    };
    assert_eq!(rc, 0);
    // CPython-exact FillInfo: allocation-free, `internal` NULL, shape/strides
    // self-referential.
    assert!(info.internal.is_null());
    assert!(std::ptr::eq(info.shape.cast_const(), &raw const info.len));

    let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromBuffer(&mut info) };
    assert!(!view.is_null());
    // The caller still owns `info` and releases it exactly once; the
    // memoryview's copied descriptor must be unaffected.
    unsafe { molt_cpython_abi::api::buffer::PyBuffer_Release(&mut info) };
    assert!(info.internal.is_null());

    assert!(unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BASE(view) }.is_null());
    let buffer = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(view) };
    assert!(!buffer.is_null());
    // The copy's descriptor VALUES live in the memoryview object's own
    // embedded storage (CPython's ob_array model): no side allocation
    // (`internal` NULL) and shape/strides/format point INTO the object.
    assert!(unsafe { (*buffer).internal }.is_null());
    let mv = view.cast::<molt_cpython_abi::abi_types::PyMemoryViewObject>();
    assert!(
        std::ptr::eq(unsafe { (*buffer).shape }.cast_const(), unsafe {
            (&raw const (*mv).ob_shape).cast()
        }),
        "FromBuffer shape must point into the object's embedded ob_shape",
    );
    assert!(
        std::ptr::eq(unsafe { (*buffer).strides }.cast_const(), unsafe {
            (&raw const (*mv).ob_strides).cast()
        }),
        "FromBuffer strides must point into the object's embedded ob_strides",
    );
    assert!(
        std::ptr::eq(unsafe { (*buffer).format }.cast_const(), unsafe {
            (&raw const (*mv).ob_format).cast()
        }),
        "FromBuffer format must point into the object's embedded ob_format",
    );
    assert_eq!(unsafe { (*buffer).buf }, bytes.as_mut_ptr().cast());
    assert_eq!(unsafe { (*buffer).len }, bytes.len() as isize);
    assert_eq!(unsafe { (*buffer).itemsize }, 1);
    assert_eq!(unsafe { (*buffer).readonly }, 1);
    assert_eq!(unsafe { (*buffer).ndim }, 1);
    unsafe {
        assert_eq!(*(*buffer).format as u8, b'B');
        assert_eq!(*(*buffer).shape, bytes.len() as isize);
        assert_eq!(*(*buffer).strides, 1);
    }
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
}

#[test]
fn test_memoryview_from_buffer_rejects_indirect_suboffsets() {
    let _guard = init();
    let mut bytes = [1_u8, 2, 3, 4];
    let mut shape = [bytes.len() as isize];
    let mut strides = [1isize];
    let mut suboffsets = [0isize];
    let mut format = [b'B' as c_char, 0];
    let mut info: Py_buffer = unsafe { std::mem::zeroed() };
    info.buf = bytes.as_mut_ptr().cast();
    info.len = bytes.len() as isize;
    info.itemsize = 1;
    info.readonly = 1;
    info.ndim = 1;
    info.format = format.as_mut_ptr();
    info.shape = shape.as_mut_ptr();
    info.strides = strides.as_mut_ptr();
    info.suboffsets = suboffsets.as_mut_ptr();

    let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromBuffer(&mut info) };
    assert!(view.is_null());
}

#[test]
fn test_memoryview_from_buffer_preserves_zero_dimensional_descriptor() {
    let _guard = init();
    let mut bytes = [0_u8; 8];
    let mut format = [b'd' as c_char, 0];
    let mut info: Py_buffer = unsafe { std::mem::zeroed() };
    info.buf = bytes.as_mut_ptr().cast();
    info.len = bytes.len() as isize;
    info.itemsize = bytes.len() as isize;
    info.readonly = 1;
    info.ndim = 0;
    info.format = format.as_mut_ptr();

    let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromBuffer(&mut info) };
    assert!(!view.is_null());
    let buffer = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(view) };
    assert!(!buffer.is_null());
    assert_eq!(unsafe { (*buffer).ndim }, 0);
    assert_eq!(unsafe { (*buffer).len }, bytes.len() as isize);
    assert_eq!(unsafe { (*buffer).itemsize }, bytes.len() as isize);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
}

#[test]
fn test_memoryview_from_buffer_ignores_foreign_private_internal_pointer() {
    let _guard = init();
    let mut bytes = [1_u8, 2, 3, 4];
    let mut shape = [bytes.len() as isize];
    let mut strides = [1isize];
    let mut format = [b'B' as c_char, 0];
    let mut info: Py_buffer = unsafe { std::mem::zeroed() };
    info.buf = bytes.as_mut_ptr().cast();
    info.len = bytes.len() as isize;
    info.itemsize = 1;
    info.readonly = 1;
    info.ndim = 1;
    info.format = format.as_mut_ptr();
    info.shape = shape.as_mut_ptr();
    info.strides = strides.as_mut_ptr();
    info.internal = std::ptr::dangling_mut::<c_void>();

    let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromBuffer(&mut info) };
    assert!(!view.is_null());
    let buffer = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(view) };
    assert!(!buffer.is_null());
    assert_ne!(unsafe { (*buffer).internal }, info.internal);
    assert_eq!(unsafe { (*buffer).len }, bytes.len() as isize);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(view) };
}

/// Constructs a memoryview over `data` in ITS OWN stack frame, so any
/// descriptor pointer that (incorrectly) targeted this frame's locals dangles
/// as soon as it returns.
#[inline(never)]
fn build_memoryview_from_memory(data: *mut c_char, len: isize) -> *mut PyObject {
    unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromMemory(data, len, PyBUF_READ) }
}

/// Same for the FromBuffer path: the source `Py_buffer` is a FillInfo'd STACK
/// view that is released and dies with this frame — exactly the shape of the
/// reverted `7da58cff8f` field-trick UAF (a self-referential `shape =
/// &view.len` on a stack view that the memoryview then outlived).
#[inline(never)]
fn build_memoryview_from_stack_buffer(data: *mut c_void, len: isize) -> *mut PyObject {
    let mut info: Py_buffer = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        molt_cpython_abi::api::buffer::PyBuffer_FillInfo(
            &mut info,
            ptr::null_mut(),
            data,
            len,
            1,
            PyBUF_FORMAT | PyBUF_STRIDES,
        )
    };
    assert_eq!(rc, 0);
    let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromBuffer(&mut info) };
    // The caller of FromBuffer keeps ownership of the original and releases it
    // exactly once — here, before the stack view goes out of scope.
    unsafe { molt_cpython_abi::api::buffer::PyBuffer_Release(&mut info) };
    view
}

/// Overwrites a stretch of stack so a dangling into-dead-frame pointer reads
/// garbage rather than accidentally-intact values on non-Miri runs.
#[inline(never)]
fn clobber_stack() -> u64 {
    let mut junk = [0u8; 4096];
    let mut acc = 0u64;
    for (i, byte) in junk.iter_mut().enumerate() {
        *byte = (i as u8) ^ 0xA5;
        acc = acc.wrapping_add(u64::from(*byte));
    }
    std::hint::black_box(acc)
}

/// Anti-dangle gate: the C-visible `shape`/`strides`/`format` pointers of a
/// memoryview must remain valid AFTER the constructing stack frame has
/// returned (they must point into the object's own storage — never into a
/// stack `Py_buffer`). Under Miri (Stacked + Tree Borrows) a dangling read
/// here is flagged deterministically; natively, `clobber_stack` makes it
/// fail loudly on values too.
#[test]
fn test_memoryview_descriptor_outlives_constructing_frame() {
    let _guard = init();
    let mut data = [7_u8; 32];

    let mv_mem = build_memoryview_from_memory(data.as_mut_ptr().cast(), data.len() as isize);
    assert!(!mv_mem.is_null());
    let mv_buf = build_memoryview_from_stack_buffer(data.as_mut_ptr().cast(), data.len() as isize);
    assert!(!mv_buf.is_null());
    std::hint::black_box(clobber_stack());

    for (label, mv) in [("FromMemory", mv_mem), ("FromBuffer", mv_buf)] {
        let buffer = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(mv) };
        assert!(!buffer.is_null(), "{label}: GET_BUFFER");
        unsafe {
            assert!(!(*buffer).shape.is_null(), "{label}: shape");
            assert!(!(*buffer).strides.is_null(), "{label}: strides");
            assert!(!(*buffer).format.is_null(), "{label}: format");
            assert_eq!(
                *(*buffer).shape,
                data.len() as isize,
                "{label}: shape[0] read after the constructing frame returned",
            );
            assert_eq!(
                *(*buffer).strides,
                1,
                "{label}: strides[0] read after the constructing frame returned",
            );
            assert_eq!(
                *(*buffer).format as u8,
                b'B',
                "{label}: format read after the constructing frame returned",
            );
            assert_eq!((*buffer).len, data.len() as isize, "{label}: len");
        }
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(mv) };
    }
}

#[test]
fn test_one_dimensional_gapped_buffer_is_not_contiguous() {
    let _guard = init();
    let mut bytes = [1_u8, 2, 3, 4, 5, 6];
    let mut shape = [3isize];
    let mut strides = [2isize];
    let mut format = [b'B' as c_char, 0];
    let mut info: Py_buffer = unsafe { std::mem::zeroed() };
    info.buf = bytes.as_mut_ptr().cast();
    info.len = 3;
    info.itemsize = 1;
    info.readonly = 1;
    info.ndim = 1;
    info.format = format.as_mut_ptr();
    info.shape = shape.as_mut_ptr();
    info.strides = strides.as_mut_ptr();

    assert_eq!(
        unsafe { molt_cpython_abi::api::buffer::PyBuffer_IsContiguous(&info, b'C' as c_char) },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::buffer::PyBuffer_IsContiguous(&info, b'F' as c_char) },
        0
    );
}

// ---------------------------------------------------------------------------
// PyObject_Hash
// ---------------------------------------------------------------------------

#[test]
fn test_object_hash_non_null() {
    let _guard = init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    let hash = unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(py) };
    // Should return some non-zero value (pointer-based)
    assert_ne!(hash, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_object_length_hint_uses_default_for_unknown_object() {
    let _guard = init();
    let hint = unsafe { molt_cpython_abi::api::object::PyObject_LengthHint(ptr::null_mut(), 17) };
    assert_eq!(hint, 17);
}

#[test]
fn test_object_self_iter_returns_new_reference_to_same_object() {
    let _guard = init();
    // A mortal carrier proves the new-reference increment; cached small ints
    // are immortal and intentionally ignore INCREF.
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1000) };
    let initial_refcnt = unsafe { (*py).ob_refcnt };
    let iter = unsafe { molt_cpython_abi::api::object::PyObject_SelfIter(py) };
    assert_eq!(iter, py);
    assert_eq!(unsafe { (*py).ob_refcnt }, initial_refcnt + 1);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(iter);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_object_hash_different_objects_differ() {
    let _guard = init();
    let a = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let b = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let ha = unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(a) };
    let hb = unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(b) };
    // Different pointers => different hashes (pointer-based hash)
    assert_ne!(ha, hb);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(a);
        molt_cpython_abi::api::refcount::Py_DECREF(b);
    }
}

// ---------------------------------------------------------------------------
// PyObject_TypeCheck
// ---------------------------------------------------------------------------

#[test]
fn test_object_typecheck_matching_type() {
    let _guard = init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let tp = unsafe { (*py).ob_type };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_TypeCheck(py, tp) };
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_object_typecheck_mismatched_type() {
    let _guard = init();
    let py_int = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let py_float = unsafe { molt_cpython_abi::api::numbers::PyFloat_FromDouble(1.0) };
    let float_tp = unsafe { (*py_float).ob_type };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_TypeCheck(py_int, float_tp) };
    assert_eq!(result, 0);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(py_int);
        molt_cpython_abi::api::refcount::Py_DECREF(py_float);
    }
}

#[test]
fn test_object_typecheck_null_args() {
    let _guard = init();
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::typeobj::PyObject_TypeCheck(ptr::null_mut(), ptr::null_mut())
        },
        0
    );
}

// ---------------------------------------------------------------------------
// PyObject_IsInstance
// ---------------------------------------------------------------------------

#[test]
fn test_isinstance_null_returns_zero() {
    let _guard = init();
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::typeobj::PyObject_IsInstance(ptr::null_mut(), ptr::null_mut())
        },
        0
    );
}

// ---------------------------------------------------------------------------
// Py_TYPE
// ---------------------------------------------------------------------------

#[test]
fn test_py_type_returns_ob_type() {
    let _guard = init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(10) };
    let tp = unsafe { molt_cpython_abi::api::typeobj::_Py_TYPE(py) };
    assert!(!tp.is_null());
    assert_eq!(tp, unsafe { (*py).ob_type });
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_py_type_null_returns_null() {
    let _guard = init();
    let tp = unsafe { molt_cpython_abi::api::typeobj::_Py_TYPE(ptr::null_mut()) };
    assert!(tp.is_null());
}

#[test]
fn test_pyobject_type_returns_new_reference_to_ob_type() {
    let _guard = init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(10) };
    let tp = unsafe { (*py).ob_type };
    let before = unsafe { (*tp).ob_base.ob_base.ob_refcnt };
    let type_obj = unsafe { molt_cpython_abi::api::typeobj::PyObject_Type(py) };

    assert_eq!(type_obj, tp.cast::<PyObject>());
    // PyObject_Type returns a NEW reference to ob_type. For an immortal builtin
    // type (PyLong_Type here) the INCREF is a permanent no-op, so the refcount is
    // UNCHANGED; a mortal type would show before+1. (Was hard-coded `before + 1`,
    // stale once builtin type statics became immortal.)
    assert_eq!(
        unsafe { (*tp).ob_base.ob_base.ob_refcnt },
        refcnt_after_one_incref(before)
    );

    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(type_obj);
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_pyobject_type_null_sets_error_and_returns_null() {
    let _guard = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let type_obj = unsafe { molt_cpython_abi::api::typeobj::PyObject_Type(ptr::null_mut()) };
    assert!(type_obj.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_pyeval_save_restore_thread_uses_singleton_thread_state() {
    let _guard = init();
    let tstate = unsafe { molt_cpython_abi::api::object::PyEval_SaveThread() };
    assert!(!tstate.is_null());
    assert!(std::ptr::eq(tstate, unsafe {
        molt_cpython_abi::api::object::PyThreadState_Get()
    }));
    unsafe { molt_cpython_abi::api::object::PyEval_RestoreThread(tstate) };
}

// Mask-proof regression for POISON Lane A #9 — PyThreadState_GetFrame theater.
// The ABI-tier PyThreadState carries no CPython frame stack, so GetFrame must
// return NULL ("no frame executing"), never a fabricated empty PyFrameObject
// that a frame-walking C extension would read as the real execution frame.
// Pre-fix this returned a non-null synthetic frame (via PyFrame_New) → FAILS;
// post-fix it returns NULL → PASSES.
#[test]
fn test_pythreadstate_getframe_returns_null_not_synthetic_frame() {
    let _guard = init();
    let tstate = unsafe { molt_cpython_abi::api::object::PyThreadState_Get() };
    assert!(
        !tstate.is_null(),
        "a valid thread state is needed for the test"
    );
    let frame = unsafe { molt_cpython_abi::api::object::PyThreadState_GetFrame(tstate) };
    assert!(
        frame.is_null(),
        "PyThreadState_GetFrame must return NULL (no CPython frame stack), \
         never a fabricated empty frame read as the real execution frame"
    );
}

#[test]
fn test_gil_check_mutex_and_unstable_unique_refs() {
    let _guard = init();

    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyGILState_Check() },
        1
    );

    let mut mutex = PyMutex { _bits: 0 };
    unsafe { molt_cpython_abi::api::object::PyMutex_Lock(&mut mutex) };
    assert_eq!(mutex._bits, 1);
    unsafe { molt_cpython_abi::api::object::PyMutex_Unlock(&mut mutex) };
    assert_eq!(mutex._bits, 0);

    let mut obj = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyUnstable_Object_IsUniquelyReferenced(&mut obj) },
        1
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::object::PyUnstable_Object_IsUniqueReferencedTemporary(&mut obj)
        },
        1
    );
    obj.ob_refcnt = 2;
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyUnstable_Object_IsUniquelyReferenced(&mut obj) },
        0
    );
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::object::PyUnstable_Object_IsUniquelyReferenced(ptr::null_mut())
        },
        0
    );

    assert_eq!(unsafe { Py_OptimizeFlag }, 0);
}

// ---------------------------------------------------------------------------
// PyCallable_Check
// ---------------------------------------------------------------------------

#[test]
fn test_callable_check_null_returns_zero() {
    let _guard = init();
    let result = unsafe { molt_cpython_abi::api::typeobj::PyCallable_Check(ptr::null_mut()) };
    assert_eq!(result, 0);
}

#[test]
fn test_callable_check_on_int_returns_zero() {
    let _guard = init();
    // Integers don't have tp_call
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(5) };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyCallable_Check(py) };
    assert_eq!(result, 0);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

unsafe extern "C" fn return_none_noargs(
    _self_: *mut PyObject,
    args: *mut PyObject,
) -> *mut PyObject {
    if !args.is_null() {
        return ptr::null_mut();
    }
    let none = &raw mut molt_cpython_abi::abi_types::Py_None;
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(none) };
    none
}

unsafe extern "C" fn echo_single_arg(_self_: *mut PyObject, arg: *mut PyObject) -> *mut PyObject {
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(arg) };
    arg
}

#[test]
fn test_cfunction_new_is_callable() {
    let _guard = init();
    static NAME: &[u8] = b"f\0";
    let mut def = PyMethodDef {
        ml_name: NAME.as_ptr().cast(),
        ml_meth: Some(return_none_noargs),
        ml_flags: METH_NOARGS,
        ml_doc: ptr::null(),
    };
    let func =
        unsafe { molt_cpython_abi::api::object::PyCFunction_New(&raw mut def, ptr::null_mut()) };
    assert!(!func.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyCFunction_Check(func) },
        1
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyCallable_Check(func) },
        1
    );

    let result = unsafe { molt_cpython_abi::api::object::PyObject_CallNoArgs(func) };
    assert!(std::ptr::eq(
        result,
        &raw mut molt_cpython_abi::abi_types::Py_None
    ));
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(result);
        molt_cpython_abi::api::refcount::Py_DECREF(func);
    }
}

#[test]
fn test_object_get_optional_attr_propagates_non_attribute_error() {
    // Under the stub table `PyUnicode_FromString` fails closed (NULL name +
    // pending MemoryError). CPython `_PyObject_LookupAttr` semantics: ONLY an
    // AttributeError means "attribute absent" (0); any other pending exception
    // must propagate as -1 with the exception preserved. The previous version of
    // this test asserted `rc == 0` with the error cleared — that green was the
    // swallow-all `PyErr_Clear()` divergence itself (ledger object.rs:293 [H]):
    // a MemoryError from the lookup path was misreported as 'attribute absent'.
    // The genuine missing-attribute→0 contract is covered by the
    // `get_optional_attr_absent_on_attribute_error` unit test on a fake type.
    let _guard = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(11) };
    let name = unsafe { molt_cpython_abi::api::strings::PyUnicode_FromString(c"missing".as_ptr()) };
    assert!(
        name.is_null(),
        "stub alloc_str must fail closed (this test exercises the error path)"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "the failed name construction must leave MemoryError pending"
    );
    let mut result: *mut PyObject = ptr::null_mut();
    let rc =
        unsafe { molt_cpython_abi::api::object::PyObject_GetOptionalAttr(py, name, &mut result) };
    assert_eq!(
        rc, -1,
        "a pending non-AttributeError must propagate as -1, never 'absent'"
    );
    assert!(result.is_null());
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "the MemoryError must stay pending (not swallowed)"
    );
    unsafe {
        molt_cpython_abi::api::errors::PyErr_Clear();
        molt_cpython_abi::api::refcount::Py_DECREF(py);
    }
}

#[test]
fn test_method_new_binds_self_for_cfunction() {
    let _guard = init();
    static NAME: &[u8] = b"echo\0";
    let mut def = PyMethodDef {
        ml_name: NAME.as_ptr().cast(),
        ml_meth: Some(echo_single_arg),
        ml_flags: METH_O,
        ml_doc: ptr::null(),
    };
    let func =
        unsafe { molt_cpython_abi::api::object::PyCFunction_New(&raw mut def, ptr::null_mut()) };
    let self_obj = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(77) };
    let method = unsafe { molt_cpython_abi::api::object::PyMethod_New(func, self_obj) };
    assert!(!method.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::object::PyMethod_Check(method) },
        1
    );
    assert!(std::ptr::eq(
        unsafe { molt_cpython_abi::api::object::PyMethod_GET_FUNCTION(method) },
        func
    ));
    assert!(std::ptr::eq(
        unsafe { molt_cpython_abi::api::object::PyMethod_GET_SELF(method) },
        self_obj
    ));

    let result = unsafe { molt_cpython_abi::api::object::PyObject_CallNoArgs(method) };
    assert!(std::ptr::eq(result, self_obj));
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(result);
        molt_cpython_abi::api::refcount::Py_DECREF(method);
        molt_cpython_abi::api::refcount::Py_DECREF(self_obj);
        molt_cpython_abi::api::refcount::Py_DECREF(func);
    }
}

// ---------------------------------------------------------------------------
// PyObject_RichCompare / PyObject_RichCompareBool
// ---------------------------------------------------------------------------

const PY_LT: i32 = 0;
const PY_EQ: i32 = 2;
const PY_NE: i32 = 3;

#[test]
fn test_richcompare_same_object_eq() {
    let _guard = init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    // Without tp_richcompare set, falls back to NotImplemented, then pointer identity
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(py, py, PY_EQ) };
    // Same pointer => EQ should be 1
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
}

#[test]
fn test_richcompare_different_objects_ne() {
    let _guard = init();
    let a = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let b = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(a, b, PY_NE) };
    // Different pointers => NE should be 1
    assert_eq!(result, 1);
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(a);
        molt_cpython_abi::api::refcount::Py_DECREF(b);
    }
}

#[test]
fn test_richcompare_native_int_ordering_is_computed() {
    let _guard = init();
    // CPython: 1 < 2 dispatches long_richcompare and yields Py_True — never
    // NotImplemented. The old ABI returned the NotImplemented sentinel here
    // (ledger typeobj.rs:1920 divergence); natives now compare by value.
    let a = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let b = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(2) };
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompare(a, b, PY_LT) };
    assert!(
        std::ptr::eq(
            result,
            (&raw mut molt_cpython_abi::abi_types::Py_True).cast::<PyObject>()
        ),
        "1 < 2 must be Py_True, never NotImplemented"
    );
    assert!(!std::ptr::eq(result, &raw mut Py_NotImplementedSentinel));
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(a);
        molt_cpython_abi::api::refcount::Py_DECREF(b);
    }
}

#[test]
fn test_richcompare_null_is_bad_internal_call() {
    let _guard = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    // CPython PyObject_RichCompare: a NULL operand is a BadInternalCall —
    // NULL return with an exception set, never a fabricated NotImplemented.
    let result = unsafe {
        molt_cpython_abi::api::typeobj::PyObject_RichCompare(
            ptr::null_mut(),
            ptr::null_mut(),
            PY_EQ,
        )
    };
    assert!(result.is_null());
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_richcomparebool_null_returns_error() {
    let _guard = init();
    let result = unsafe {
        molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(
            ptr::null_mut(),
            ptr::null_mut(),
            PY_LT,
        )
    };
    // LT on null => cannot compare => -1 (error)
    assert_eq!(result, -1);
}

// ---------------------------------------------------------------------------
// PyObject_Dir — fail-open burndown teeth
// ---------------------------------------------------------------------------

#[test]
fn test_object_dir_null_fails_closed() {
    // F6 teeth: PyObject_Dir previously returned an empty list ignoring `o`.
    // PyObject_Dir(NULL) (frame-local dir) is unsupported from the ABI bridge and
    // must fail closed with NULL + an exception, never an empty-list placeholder.
    let _guard = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let result = unsafe { molt_cpython_abi::api::object::PyObject_Dir(ptr::null_mut()) };
    assert!(
        result.is_null(),
        "PyObject_Dir(NULL) must fail closed (NULL), not return an empty list"
    );
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "a NULL return from PyObject_Dir must leave an exception set"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

#[test]
fn test_object_dir_foreign_nonbridge_does_not_hang() {
    // CPYTHON-ABI-LOCK-SWEEP regression test: PyObject_Dir(o) with a non-NULL,
    // non-bridge-managed `o` takes the `None` arm of what was
    // `match GLOBAL_BRIDGE...`. That arm calls PyErr_SetString, which
    // itself locks GLOBAL_BRIDGE — a self-deadlock (hang, not a crash) on a
    // non-reentrant Mutex. Reproduced live against the pre-fix code (the
    // spawned thread below hung past the 10s bound); the fix binds the lock's
    // result to a local *before* the match so the guard drops before any arm
    // runs. Runs the call in a spawned thread with a bounded join so a
    // regression fails this test instead of wedging the whole suite.
    let _guard = init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let mut fake = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };
    let raw_addr = (&raw mut fake) as usize;
    let handle = std::thread::spawn(move || {
        let o = raw_addr as *mut PyObject;
        (unsafe { molt_cpython_abi::api::object::PyObject_Dir(o) }) as usize
    });
    let start = std::time::Instant::now();
    loop {
        if handle.is_finished() {
            let result = handle.join().expect("PyObject_Dir thread panicked");
            assert_eq!(
                result, 0,
                "PyObject_Dir on a foreign object must fail closed (NULL)"
            );
            unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
            break;
        }
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "PyObject_Dir(foreign) HUNG for >10s — GLOBAL_BRIDGE self-deadlock reproduced"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn test_object_delitem_null_returns_error() {
    // PyObject_DelItem now routes real deletion through the runtime dict_del
    // authority (previously it set the key to None — not deletion). NULL args are
    // the error sentinel -1.
    let _guard = init();
    let rc = unsafe {
        molt_cpython_abi::api::object::PyObject_DelItem(ptr::null_mut(), ptr::null_mut())
    };
    assert_eq!(rc, -1);
}
