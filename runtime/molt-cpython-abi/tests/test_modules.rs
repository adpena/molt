//! Tests for PyModule_New, PyModule_GetDict, PyModule_Create2,
//! PyModule_AddObject, PyModule_AddIntConstant, PyModule_AddStringConstant,
//! PyModuleDef_Init.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::{MoltBufferView, RuntimeHooks};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

// ─── Test hook implementations ───────────────────────────────────────────────
//
// `molt-lang-cpython-abi` deliberately does not depend on `molt-lang-runtime`
// (avoids a circular dep), so integration tests in this crate cannot pull in
// the real runtime's hook implementations.  Instead we install a minimal
// counter-backed vtable that hands out monotonically increasing non-zero
// "handle bits" — enough for `PyModule_New` / `PyModule_Create2` to return a
// non-null wrapped `*mut PyObject` so the bridge logic itself can be exercised.
//
// The real runtime overrides this in production via
// `molt_cpython_abi_register_hooks`.

static FAKE_HANDLE_COUNTER: AtomicU64 = AtomicU64::new(0x1000);
static FAKE_BUFFER_RELEASES: AtomicU64 = AtomicU64::new(0);
static FAKE_BUFFER_LOCK: Mutex<()> = Mutex::new(());
static FAKE_BUFFER: [u8; 4] = [1, 2, 3, 4];

fn next_fake_handle() -> u64 {
    // NaN-boxed pointers are 50-bit aligned to ≥2-byte boundaries; bumping
    // by 8 keeps the sequence well clear of inline-int / inline-bool / None
    // bit patterns and stays inside the heap-pointer space.
    FAKE_HANDLE_COUNTER.fetch_add(8, Ordering::Relaxed)
}

unsafe extern "C" fn fake_alloc_str(_data: *const u8, _len: usize) -> u64 {
    next_fake_handle()
}
unsafe extern "C" fn fake_alloc_bytes(_data: *const u8, _len: usize) -> u64 {
    next_fake_handle()
}
unsafe extern "C" fn fake_int_from_i64(_value: i64) -> u64 {
    next_fake_handle()
}
unsafe extern "C" fn fake_int_from_u64(_value: u64) -> u64 {
    next_fake_handle()
}
unsafe extern "C" fn fake_int_as_i64(_bits: u64) -> i64 {
    -1
}
unsafe extern "C" fn fake_alloc_list() -> u64 {
    next_fake_handle()
}
unsafe extern "C" fn fake_list_append(_list_bits: u64, _item_bits: u64) {}
unsafe extern "C" fn fake_list_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn fake_list_item(_bits: u64, _i: usize) -> u64 {
    0
}
unsafe extern "C" fn fake_alloc_tuple(_arity: usize) -> u64 {
    next_fake_handle()
}
unsafe extern "C" fn fake_tuple_set(_bits: u64, _i: usize, _value: u64) {}
unsafe extern "C" fn fake_tuple_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn fake_tuple_item(_bits: u64, _i: usize) -> u64 {
    0
}
unsafe extern "C" fn fake_alloc_dict() -> u64 {
    next_fake_handle()
}
unsafe extern "C" fn fake_dict_set(_d: u64, _k: u64, _v: u64) {}
unsafe extern "C" fn fake_dict_get(_d: u64, _k: u64) -> u64 {
    0
}
unsafe extern "C" fn fake_dict_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn fake_str_data(_bits: u64, out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe {
            *out_len = 0;
        }
    }
    b"".as_ptr()
}
unsafe extern "C" fn fake_bytes_data(_bits: u64, out_len: *mut usize) -> *const u8 {
    if !out_len.is_null() {
        unsafe {
            *out_len = 0;
        }
    }
    std::ptr::null()
}
unsafe extern "C" fn fake_buffer_acquire(
    bits: u64,
    out_view: *mut MoltBufferView,
) -> std::os::raw::c_int {
    if bits == 0 || out_view.is_null() {
        return -1;
    }
    unsafe {
        *out_view = MoltBufferView::default();
        (*out_view).data = FAKE_BUFFER.as_ptr() as *mut u8;
        (*out_view).len = FAKE_BUFFER.len() as u64;
        (*out_view).readonly = 0;
        (*out_view).ndim = 2;
        (*out_view).itemsize = 1;
        (*out_view).owner = bits;
        (*out_view).base = bits;
        (*out_view).shape[0] = 2;
        (*out_view).shape[1] = 2;
        (*out_view).strides[0] = 2;
        (*out_view).strides[1] = 1;
        (*out_view).format[0] = b'B';
        (*out_view).format[1] = 0;
    }
    0
}
unsafe extern "C" fn fake_buffer_release(view: *mut MoltBufferView) -> std::os::raw::c_int {
    FAKE_BUFFER_RELEASES.fetch_add(1, Ordering::Relaxed);
    if !view.is_null() {
        unsafe {
            *view = MoltBufferView::default();
        }
    }
    0
}
unsafe extern "C" fn fake_classify_heap(_bits: u64) -> u8 {
    MoltTypeTag::Other as u8
}
unsafe extern "C" fn fake_inc_ref(_bits: u64) {}
unsafe extern "C" fn fake_dec_ref(_bits: u64) {}
unsafe extern "C" fn fake_alloc_module(_data: *const u8, _len: usize) -> u64 {
    next_fake_handle()
}
unsafe extern "C" fn fake_module_set_attr(
    _m: u64,
    _data: *const u8,
    _len: usize,
    _v: u64,
) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn fake_register_c_function(
    _meth: u64,
    _flags: std::os::raw::c_int,
    _data: *const u8,
    _len: usize,
) -> u64 {
    next_fake_handle()
}

const TEST_HOOKS: RuntimeHooks = RuntimeHooks {
    alloc_str: fake_alloc_str,
    alloc_bytes: fake_alloc_bytes,
    int_from_i64: fake_int_from_i64,
    int_from_u64: fake_int_from_u64,
    int_as_i64: fake_int_as_i64,
    alloc_list: fake_alloc_list,
    list_append: fake_list_append,
    list_len: fake_list_len,
    list_item: fake_list_item,
    alloc_tuple: fake_alloc_tuple,
    tuple_set: fake_tuple_set,
    tuple_len: fake_tuple_len,
    tuple_item: fake_tuple_item,
    alloc_dict: fake_alloc_dict,
    dict_set: fake_dict_set,
    dict_get: fake_dict_get,
    dict_len: fake_dict_len,
    str_data: fake_str_data,
    bytes_data: fake_bytes_data,
    buffer_acquire: fake_buffer_acquire,
    buffer_release: fake_buffer_release,
    classify_heap: fake_classify_heap,
    inc_ref: fake_inc_ref,
    dec_ref: fake_dec_ref,
    alloc_module: fake_alloc_module,
    module_set_attr: fake_module_set_attr,
    register_c_function: fake_register_c_function,
};

fn init() {
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    // Idempotent — only the first test in the run actually installs hooks;
    // subsequent calls observe the already-registered state and silently
    // no-op rather than panicking on `OnceLock::set` failure.
    unsafe {
        molt_cpython_abi::try_set_runtime_hooks(TEST_HOOKS);
    }
}

#[test]
fn test_getbuffer_uses_runtime_typed_descriptor() {
    let _guard = FAKE_BUFFER_LOCK.lock().unwrap();
    init();
    let obj = unsafe {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .lock()
            .handle_to_pyobj(next_fake_handle())
    };
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    let flags = 0x0004 | 0x0010 | 0x0008;
    let rc = unsafe { molt_cpython_abi::api::buffer::PyObject_GetBuffer(obj, &mut view, flags) };
    assert_eq!(rc, 0);
    assert_eq!(view.len, 4);
    assert_eq!(view.itemsize, 1);
    assert_eq!(view.readonly, 0);
    assert_eq!(view.ndim, 2);
    assert!(!view.buf.is_null());
    assert!(!view.format.is_null());
    assert!(!view.shape.is_null());
    assert!(!view.strides.is_null());
    unsafe {
        assert_eq!(*view.format as u8, b'B');
        assert_eq!(*view.shape.add(0), 2);
        assert_eq!(*view.shape.add(1), 2);
        assert_eq!(*view.strides.add(0), 2);
        assert_eq!(*view.strides.add(1), 1);
        assert_eq!(
            molt_cpython_abi::api::buffer::PyBuffer_IsContiguous(&view, b'C' as _),
            1
        );
        molt_cpython_abi::api::buffer::PyBuffer_Release(&mut view);
        assert!(view.buf.is_null());
        assert!(view.internal.is_null());
        molt_cpython_abi::api::refcount::Py_DECREF(obj);
    }
}

#[test]
fn test_memoryview_uses_runtime_buffer_lifetime() {
    let _guard = FAKE_BUFFER_LOCK.lock().unwrap();
    init();
    FAKE_BUFFER_RELEASES.store(0, Ordering::Relaxed);
    let obj = unsafe {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .lock()
            .handle_to_pyobj(next_fake_handle())
    };
    let rc_before = unsafe { (*obj).ob_refcnt };
    let memoryview = unsafe { molt_cpython_abi::api::memory::PyMemoryView_FromObject(obj) };
    assert!(!memoryview.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::memory::PyMemoryView_Check(memoryview) },
        1
    );
    assert_eq!(unsafe { (*obj).ob_refcnt }, rc_before + 1);

    let view = unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BUFFER(memoryview) };
    assert!(!view.is_null());
    unsafe {
        assert_eq!((*view).len, 4);
        assert_eq!((*view).itemsize, 1);
        assert_eq!((*view).readonly, 0);
        assert_eq!((*view).ndim, 2);
        assert_eq!(*(*view).shape.add(0), 2);
        assert_eq!(*(*view).shape.add(1), 2);
        assert_eq!(*(*view).strides.add(0), 2);
        assert_eq!(*(*view).strides.add(1), 1);
    }
    assert_eq!(
        unsafe { molt_cpython_abi::api::memory::PyMemoryView_GET_BASE(memoryview) },
        obj
    );

    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(memoryview) };
    assert_eq!(FAKE_BUFFER_RELEASES.load(Ordering::Relaxed), 1);
    assert_eq!(unsafe { (*obj).ob_refcnt }, rc_before);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(obj) };
}

// ---------------------------------------------------------------------------
// PyModule_New
// ---------------------------------------------------------------------------

#[test]
fn test_module_new_non_null() {
    init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(c"testmod".as_ptr()) };
    assert!(!m.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

#[test]
fn test_module_new_null_name_returns_null() {
    init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(ptr::null()) };
    assert!(m.is_null());
}

// ---------------------------------------------------------------------------
// PyModule_GetDict
// ---------------------------------------------------------------------------

#[test]
fn test_module_getdict_non_null() {
    init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(c"mod".as_ptr()) };
    let d = unsafe { molt_cpython_abi::api::modules::PyModule_GetDict(m) };
    // Returns the module itself as a placeholder
    assert!(!d.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

#[test]
fn test_module_getdict_null_returns_null() {
    init();
    let d = unsafe { molt_cpython_abi::api::modules::PyModule_GetDict(ptr::null_mut()) };
    assert!(d.is_null());
}

// ---------------------------------------------------------------------------
// PyModule_AddObject
// ---------------------------------------------------------------------------

#[test]
fn test_module_addobject_null_module_returns_error() {
    init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe {
        molt_cpython_abi::api::modules::PyModule_AddObject(ptr::null_mut(), c"attr".as_ptr(), val)
    };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(val) };
}

#[test]
fn test_module_addobject_null_name_returns_error() {
    init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(c"mod".as_ptr()) };
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe { molt_cpython_abi::api::modules::PyModule_AddObject(m, ptr::null(), val) };
    assert_eq!(result, -1);
    // val ref was not stolen on error, clean up
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(val);
        molt_cpython_abi::api::refcount::Py_DECREF(m);
    }
}

#[test]
fn test_module_addobject_null_value_returns_error() {
    init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(c"mod".as_ptr()) };
    let result = unsafe {
        molt_cpython_abi::api::modules::PyModule_AddObject(m, c"attr".as_ptr(), ptr::null_mut())
    };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

// ---------------------------------------------------------------------------
// PyModule_AddIntConstant
// ---------------------------------------------------------------------------

#[test]
fn test_module_addintconstant_null_module() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::modules::PyModule_AddIntConstant(ptr::null_mut(), c"X".as_ptr(), 42)
    };
    assert_eq!(result, -1);
}

// ---------------------------------------------------------------------------
// PyModule_AddStringConstant
// ---------------------------------------------------------------------------

#[test]
fn test_module_addstringconstant_null_module() {
    init();
    let result = unsafe {
        molt_cpython_abi::api::modules::PyModule_AddStringConstant(
            ptr::null_mut(),
            c"Y".as_ptr(),
            c"val".as_ptr(),
        )
    };
    assert_eq!(result, -1);
}

// ---------------------------------------------------------------------------
// PyModuleDef_Init
// ---------------------------------------------------------------------------

#[test]
fn test_moduledef_init_null_returns_null() {
    init();
    let result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(ptr::null_mut()) };
    assert!(result.is_null());
}

// ---------------------------------------------------------------------------
// PyModule_Create2
// ---------------------------------------------------------------------------

#[test]
fn test_module_create2_null_returns_null() {
    init();
    let result = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(ptr::null_mut(), 0) };
    assert!(result.is_null());
}

#[test]
fn test_module_create2_with_valid_def() {
    init();
    let mut def = PyModuleDef {
        m_base: PyModuleDef_Base {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: ptr::null_mut(),
            },
            m_init: None,
            m_index: 0,
            m_copy: ptr::null_mut(),
        },
        m_name: c"testmod2".as_ptr(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: ptr::null_mut(),
        m_slots: ptr::null_mut(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(&mut def, 1013) };
    assert!(!m.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

#[test]
fn test_module_create2_null_name_uses_unnamed() {
    init();
    let mut def = PyModuleDef {
        m_base: PyModuleDef_Base {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: ptr::null_mut(),
            },
            m_init: None,
            m_index: 0,
            m_copy: ptr::null_mut(),
        },
        m_name: ptr::null(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: ptr::null_mut(),
        m_slots: ptr::null_mut(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(&mut def, 1013) };
    assert!(!m.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}
