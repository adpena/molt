//! Tests for object protocol: PyObject_Repr, Str, Hash, RichCompare,
//! TypeCheck, IsInstance, CallableCheck.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::{
    METH_NOARGS, METH_O, Py_buffer, Py_ssize_t, PyMethodDef, PyObject, is_immortal_refcnt,
};
use std::os::raw::c_char;
use std::ptr;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// Serialize process-global C views while their non-atomic reference counts
/// are accessed. Runtime value comparisons live in cpython_abi_hooks::inquiry_tests.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the binary-wide serialization guard (poison-tolerant) and run the
/// idempotent ABI init. Every test retains both guards; tuple field order
/// retires the ABI transaction before releasing the fixture serialization lock.
#[must_use = "retain ABI and fixture-lock custody for the whole test"]
fn init() -> (
    support::AbiTestThreadStateTransaction,
    MutexGuard<'static, ()>,
) {
    let guard = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let transaction = support::enter_abi_test(support::stub_runtime_hooks());
    (transaction, guard)
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
fn test_object_length_hint_rejects_null_with_an_error() {
    let _guard = init();
    let hint = unsafe { molt_cpython_abi::api::object::PyObject_LengthHint(ptr::null_mut(), 17) };
    assert_eq!(hint, -1);
    assert!(!unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
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
fn test_classinfo_null_operands_raise_system_error() {
    let _guard = init();
    use molt_cpython_abi::abi_types::{PyBaseObject_Type, PyExc_SystemError};
    use molt_cpython_abi::api::{errors, object, typeobj};
    for query in [
        typeobj::PyObject_IsInstance
            as unsafe extern "C" fn(*mut PyObject, *mut PyObject) -> std::ffi::c_int,
        object::PyObject_IsSubclass,
    ] {
        for (value, classinfo) in [
            (ptr::null_mut(), ptr::null_mut()),
            (ptr::null_mut(), (&raw mut PyBaseObject_Type).cast()),
            ((&raw mut PyBaseObject_Type).cast(), ptr::null_mut()),
        ] {
            unsafe {
                assert_eq!(query(value, classinfo), -1);
                assert_eq!(
                    errors::PyErr_Occurred(),
                    (&raw mut PyExc_SystemError).cast()
                );
                errors::PyErr_Clear();
            }
        }
    }
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

#[test]
fn test_richcompare_same_object_eq() {
    let _guard = init();
    let py = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(42) };
    // RichCompareBool accepts pointer identity before runtime value dispatch.
    let result = unsafe { molt_cpython_abi::api::typeobj::PyObject_RichCompareBool(py, py, PY_EQ) };
    // Same pointer => EQ should be 1
    assert_eq!(result, 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(py) };
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
    // The child owns the fixture transaction and its physical input. Holding
    // a parent ABI transaction here would block the child's exclusive entry;
    // lending parent stack storage would also become unsafe on timeout.
    let _guard = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let handle = std::thread::spawn(move || {
        let _abi_test = support::enter_abi_test(support::stub_runtime_hooks());
        let mut fake = PyObject {
            ob_refcnt: 1,
            ob_type: ptr::null_mut(),
        };
        let o = &raw mut fake;
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

thread_local! {
    static METHOD_VECTOR_FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static METHOD_VECTOR_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
unsafe extern "C" fn method_vector_probe(
    _function: *mut PyObject,
    args: *mut *mut PyObject,
    nargsf: usize,
    names: *mut PyObject,
) -> *mut PyObject {
    assert!(names.is_null());
    assert_eq!(
        nargsf,
        METHOD_VECTOR_COUNT.get() + 1,
        "receiver is prepended and scratch permission consumed"
    );
    unsafe {
        for index in 1..nargsf {
            assert_eq!(*args.add(index), *args);
        }
        if METHOD_VECTOR_FAIL.get() {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                c"method vector failure".as_ptr(),
            );
            ptr::null_mut()
        } else {
            molt_cpython_abi::api::refcount::Py_INCREF(*args);
            *args
        }
    }
}

#[test]
fn physical_method_vectorcall_restores_scratch_and_handles_inline_spill_and_failure() {
    use molt_cpython_abi::abi_types::{Py_TPFLAGS_HAVE_VECTORCALL, PyTypeObject, PyVectorcallFunc};
    use molt_cpython_abi::api::{errors, object, refcount};
    let _guard = init();
    #[repr(C)]
    struct Target {
        object: PyObject,
        vector: Option<PyVectorcallFunc>,
    }
    unsafe {
        let mut kind: PyTypeObject = std::mem::zeroed();
        kind.tp_name = c"method_vector_target".as_ptr();
        kind.tp_flags = Py_TPFLAGS_HAVE_VECTORCALL;
        kind.tp_vectorcall_offset = std::mem::offset_of!(Target, vector) as isize;
        let mut target = Target {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut kind,
            },
            vector: Some(method_vector_probe),
        };
        let receiver = molt_cpython_abi::api::numbers::PyLong_FromLong(77);
        let method = object::PyMethod_New(&raw mut target.object, receiver);
        assert!(!method.is_null());
        assert!(object::PyVectorcall_Function(method).is_some());
        assert!(
            (*method.cast::<molt_cpython_abi::abi_types::PyMethodObject>())
                .im_weakreflist
                .is_null()
        );
        assert_eq!(
            (*(*method).ob_type).tp_weaklistoffset,
            0,
            "native weakref support is not advertised"
        );
        let offset = 1usize << (usize::BITS - 1);
        for fail in [false, true] {
            for count in [0, 1, 8, 17] {
                for scratch in [false, true] {
                    METHOD_VECTOR_FAIL.set(fail);
                    METHOD_VECTOR_COUNT.set(count);
                    let sentinel = &raw mut target.object;
                    let mut values = vec![receiver; count + 1];
                    values[0] = sentinel;
                    let result = object::PyObject_Vectorcall(
                        method,
                        values.as_mut_ptr().add(1),
                        count | if scratch { offset } else { 0 },
                        ptr::null_mut(),
                    );
                    assert_eq!(
                        values[0], sentinel,
                        "scratch restored: count={count}, failure={fail}"
                    );
                    if fail {
                        assert!(result.is_null());
                        assert!(!errors::PyErr_Occurred().is_null());
                        errors::PyErr_Clear();
                    } else {
                        assert_eq!(result, receiver);
                        refcount::Py_DECREF(result);
                    }
                }
            }
        }
        assert!(object::PyMethod_New(&raw mut target.object, ptr::null_mut()).is_null());
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        refcount::Py_DECREF(method);
        assert_eq!(target.object.ob_refcnt, 1);
        refcount::Py_DECREF(receiver);
    }
}

thread_local! {
    static METHOD_RETIRED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
unsafe extern "C" fn method_operand_finalizer(object: *mut PyObject) {
    METHOD_RETIRED.set(METHOD_RETIRED.get() + 1);
    unsafe {
        molt_cpython_abi::api::errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast(),
            c"operand finalizer must not replace caller error".as_ptr(),
        );
        drop(Box::from_raw(object));
    }
}

#[test]
fn physical_method_retirement_preserves_pending_error_across_operand_finalizers() {
    use molt_cpython_abi::api::{errors, object, refcount};
    let _guard = init();
    unsafe {
        let mut kind: molt_cpython_abi::abi_types::PyTypeObject = std::mem::zeroed();
        kind.tp_name = c"method_owned_operand".as_ptr();
        kind.tp_dealloc = Some(method_operand_finalizer);
        let function = Box::into_raw(Box::new(PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut kind,
        }));
        let receiver = Box::into_raw(Box::new(PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut kind,
        }));
        let method = object::PyMethod_New(function, receiver);
        assert!(!method.is_null());
        refcount::Py_DECREF(function);
        refcount::Py_DECREF(receiver);
        errors::PyErr_SetString(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
            c"original method caller error".as_ptr(),
        );
        let original = errors::PyErr_GetRaisedException();
        assert!(!original.is_null());
        errors::PyErr_SetRaisedException(original);
        METHOD_RETIRED.set(0);
        refcount::Py_DECREF(method);
        assert_eq!(METHOD_RETIRED.get(), 2);
        let preserved = errors::PyErr_GetRaisedException();
        assert_eq!(preserved, original);
        refcount::Py_DECREF(preserved);
    }
}
