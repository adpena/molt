//! Tests for PyModule_New, PyModule_GetDict, PyModule_Create2,
//! PyModule_AddObject, PyModule_AddIntConstant, PyModule_AddStringConstant,
//! PyModuleDef_Init.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::{MoltBufferView, RuntimeHooks};
use std::collections::HashMap;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

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
static MODULE_EXEC_CALLED: AtomicU64 = AtomicU64::new(0);
static MODULE_EXEC_STATE_BYTE: AtomicU64 = AtomicU64::new(0);
static FAKE_MODULE_EXEC_LOCK: Mutex<()> = Mutex::new(());
static FAKE_BUFFER_LOCK: Mutex<()> = Mutex::new(());
static FAKE_BUFFER: [u8; 4] = [1, 2, 3, 4];
static FAKE_MODULE_STATE: LazyLock<Mutex<FakeModuleState>> =
    LazyLock::new(|| Mutex::new(FakeModuleState::default()));

#[derive(Default)]
struct FakeModuleState {
    dict_by_module: HashMap<u64, u64>,
    capi_by_module: HashMap<u64, FakeModuleCapi>,
    by_def: HashMap<usize, u64>,
}

#[derive(Default)]
struct FakeModuleCapi {
    state: Option<Box<[u8]>>,
}

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
unsafe extern "C" fn fake_int_as_i64_checked(_bits: u64, out: *mut i64) -> std::os::raw::c_int {
    if !out.is_null() {
        unsafe {
            *out = -1;
        }
    }
    0
}
unsafe extern "C" fn fake_int_as_u64_checked(_bits: u64, out: *mut u64) -> std::os::raw::c_int {
    if !out.is_null() {
        unsafe {
            *out = 0;
        }
    }
    0
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
unsafe extern "C" fn fake_dict_del(_d: u64, _k: u64) -> std::os::raw::c_int {
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
unsafe extern "C" fn fake_object_get_attr(_obj: u64, _name: u64) -> u64 {
    0
}
unsafe extern "C" fn fake_object_set_attr(
    _obj: u64,
    _name: u64,
    _value: u64,
) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn fake_object_format(_obj: u64, _spec: u64) -> u64 {
    next_fake_handle()
}
unsafe extern "C" fn fake_sys_get_object_borrowed(_data: *const u8, _len: usize) -> u64 {
    0
}
unsafe extern "C" fn fake_classify_heap(_bits: u64) -> u8 {
    MoltTypeTag::Other as u8
}
unsafe extern "C" fn fake_inc_ref(_bits: u64) {}
unsafe extern "C" fn fake_dec_ref(_bits: u64) {}
unsafe extern "C" fn fake_alloc_module(_data: *const u8, _len: usize) -> u64 {
    let module_bits = next_fake_handle();
    let dict_bits = next_fake_handle();
    FAKE_MODULE_STATE
        .lock()
        .unwrap()
        .dict_by_module
        .insert(module_bits, dict_bits);
    module_bits
}
unsafe extern "C" fn fake_module_get_dict(module_bits: u64) -> u64 {
    FAKE_MODULE_STATE
        .lock()
        .unwrap()
        .dict_by_module
        .get(&module_bits)
        .copied()
        .unwrap_or(0)
}
unsafe extern "C" fn fake_module_set_attr(
    _m: u64,
    data: *const u8,
    len: usize,
    _v: u64,
) -> std::os::raw::c_int {
    if !data.is_null() {
        let name = unsafe { std::slice::from_raw_parts(data, len) };
        if name == b"reject_attr" {
            return -1;
        }
    }
    0
}
unsafe extern "C" fn fake_module_capi_register(
    module_bits: u64,
    _module_def_ptr: usize,
    module_state_size: u64,
) -> std::os::raw::c_int {
    let Ok(size) = usize::try_from(module_state_size) else {
        return -1;
    };
    let state = if size == 0 {
        None
    } else {
        Some(vec![0; size].into_boxed_slice())
    };
    let mut guard = FAKE_MODULE_STATE.lock().unwrap();
    if guard.capi_by_module.contains_key(&module_bits) {
        return -1;
    }
    guard
        .capi_by_module
        .insert(module_bits, FakeModuleCapi { state });
    0
}
unsafe extern "C" fn fake_module_capi_get_state(module_bits: u64) -> *mut u8 {
    FAKE_MODULE_STATE
        .lock()
        .unwrap()
        .capi_by_module
        .get_mut(&module_bits)
        .and_then(|entry| entry.state.as_mut())
        .map_or(ptr::null_mut(), |state| state.as_mut_ptr())
}
unsafe extern "C" fn fake_module_state_add(
    module_bits: u64,
    module_def_ptr: usize,
) -> std::os::raw::c_int {
    if module_def_ptr == 0 {
        return -1;
    }
    FAKE_MODULE_STATE
        .lock()
        .unwrap()
        .by_def
        .insert(module_def_ptr, module_bits);
    0
}
unsafe extern "C" fn fake_module_state_find(module_def_ptr: usize) -> u64 {
    FAKE_MODULE_STATE
        .lock()
        .unwrap()
        .by_def
        .get(&module_def_ptr)
        .copied()
        .unwrap_or(0)
}
unsafe extern "C" fn fake_module_state_remove(module_def_ptr: usize) -> std::os::raw::c_int {
    if FAKE_MODULE_STATE
        .lock()
        .unwrap()
        .by_def
        .remove(&module_def_ptr)
        .is_some()
    {
        0
    } else {
        -1
    }
}
unsafe extern "C" fn fake_register_c_function(
    _meth: u64,
    _flags: std::os::raw::c_int,
    _self_bits: u64,
    data: *const u8,
    len: usize,
) -> u64 {
    if !data.is_null() {
        let name = unsafe { std::slice::from_raw_parts(data, len) };
        if name == b"reject" {
            return 0;
        }
    }
    next_fake_handle()
}

const TEST_HOOKS: RuntimeHooks = RuntimeHooks {
    alloc_str: fake_alloc_str,
    alloc_bytes: fake_alloc_bytes,
    int_from_i64: fake_int_from_i64,
    int_from_u64: fake_int_from_u64,
    int_as_i64: fake_int_as_i64,
    int_as_i64_checked: fake_int_as_i64_checked,
    int_as_u64_checked: fake_int_as_u64_checked,
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
    dict_del: fake_dict_del,
    dict_len: fake_dict_len,
    str_data: fake_str_data,
    bytes_data: fake_bytes_data,
    buffer_acquire: fake_buffer_acquire,
    buffer_release: fake_buffer_release,
    object_get_attr: fake_object_get_attr,
    object_set_attr: fake_object_set_attr,
    object_format: fake_object_format,
    sys_get_object_borrowed: fake_sys_get_object_borrowed,
    classify_heap: fake_classify_heap,
    inc_ref: fake_inc_ref,
    dec_ref: fake_dec_ref,
    alloc_module: fake_alloc_module,
    module_get_dict: fake_module_get_dict,
    module_set_attr: fake_module_set_attr,
    module_capi_register: fake_module_capi_register,
    module_capi_get_state: fake_module_capi_get_state,
    module_state_add: fake_module_state_add,
    module_state_find: fake_module_state_find,
    module_state_remove: fake_module_state_remove,
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
    let flags = PyBUF_FORMAT | PyBUF_STRIDES;
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
fn test_fillinfo_uses_typed_descriptor_without_runtime_release() {
    let _guard = FAKE_BUFFER_LOCK.lock().unwrap();
    init();
    FAKE_BUFFER_RELEASES.store(0, Ordering::Relaxed);
    let mut data = [9_u8, 8, 7, 6];
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };
    let flags = PyBUF_FORMAT | PyBUF_STRIDES;

    let rc = unsafe {
        molt_cpython_abi::api::buffer::PyBuffer_FillInfo(
            &mut view,
            ptr::null_mut(),
            data.as_mut_ptr().cast(),
            data.len() as isize,
            1,
            flags,
        )
    };

    assert_eq!(rc, 0);
    assert_eq!(view.buf, data.as_mut_ptr().cast());
    assert_eq!(view.len, 4);
    assert_eq!(view.itemsize, 1);
    assert_eq!(view.readonly, 1);
    assert_eq!(view.ndim, 1);
    assert!(view.obj.is_null());
    assert!(!view.internal.is_null());
    assert!(!view.format.is_null());
    assert!(!view.shape.is_null());
    assert!(!view.strides.is_null());
    unsafe {
        assert_eq!(*view.format as u8, b'B');
        assert_eq!(*view.shape, 4);
        assert_eq!(*view.strides, 1);
        molt_cpython_abi::api::buffer::PyBuffer_Release(&mut view);
    }
    assert_eq!(FAKE_BUFFER_RELEASES.load(Ordering::Relaxed), 0);
    assert!(view.buf.is_null());
    assert!(view.internal.is_null());
}

#[test]
fn test_fillinfo_rejects_writable_request_for_readonly_raw_buffer() {
    init();
    let mut data = [1_u8];
    let mut view: Py_buffer = unsafe { std::mem::zeroed() };

    let rc = unsafe {
        molt_cpython_abi::api::buffer::PyBuffer_FillInfo(
            &mut view,
            ptr::null_mut(),
            data.as_mut_ptr().cast(),
            data.len() as isize,
            1,
            PyBUF_WRITABLE,
        )
    };

    assert_eq!(rc, -1);
    assert!(view.internal.is_null());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
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

#[test]
fn test_moduledef_init_returns_definition_pointer() {
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
        m_name: c"moduledef_init_module".as_ptr(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: ptr::null_mut(),
        m_slots: ptr::null_mut(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };

    let out = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(&mut def) };

    assert_eq!(out.cast::<PyModuleDef>(), &mut def as *mut PyModuleDef);
    assert_eq!(unsafe { (*out).ob_refcnt }, 1);
    assert!(std::ptr::eq(
        unsafe { (*out).ob_type },
        &raw mut PyModuleDef_Type
    ));
}

unsafe extern "C" fn fake_module_exec(module: *mut PyObject) -> std::os::raw::c_int {
    if module.is_null() {
        return -1;
    }
    MODULE_EXEC_CALLED.fetch_add(1, Ordering::Relaxed);
    0
}

unsafe extern "C" fn fake_module_exec_failure(module: *mut PyObject) -> std::os::raw::c_int {
    if module.is_null() {
        return -1;
    }
    MODULE_EXEC_CALLED.fetch_add(1, Ordering::Relaxed);
    -1
}

unsafe extern "C" fn fake_module_exec_mutates_state(module: *mut PyObject) -> std::os::raw::c_int {
    if module.is_null() {
        return -1;
    }
    let state = unsafe { molt_cpython_abi::api::modules::PyModule_GetState(module) };
    if state.is_null() {
        return -1;
    }
    unsafe {
        *(state as *mut u8) = 77;
    }
    MODULE_EXEC_CALLED.fetch_add(1, Ordering::Relaxed);
    MODULE_EXEC_STATE_BYTE.store(77, Ordering::Relaxed);
    0
}

#[test]
fn test_module_from_def_and_spec_runs_py_mod_exec_slot() {
    let _guard = FAKE_MODULE_EXEC_LOCK.lock().unwrap();
    init();
    MODULE_EXEC_CALLED.store(0, Ordering::Relaxed);
    let mut slots = [
        PyModuleDef_Slot {
            slot: 2,
            value: fake_module_exec as *mut c_void,
        },
        PyModuleDef_Slot {
            slot: 0,
            value: ptr::null_mut(),
        },
    ];
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
        m_name: c"moduledef_exec_module".as_ptr(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: ptr::null_mut(),
        m_slots: slots.as_mut_ptr(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };

    let module = unsafe {
        molt_cpython_abi::api::modules::PyModule_FromDefAndSpec2(&mut def, ptr::null_mut(), 0)
    };

    assert!(!module.is_null());
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module) };
}

#[test]
fn test_module_from_def_and_spec_accepts_python312_metadata_slots() {
    let _guard = FAKE_MODULE_EXEC_LOCK.lock().unwrap();
    init();
    MODULE_EXEC_CALLED.store(0, Ordering::Relaxed);
    let mut slots = [
        PyModuleDef_Slot {
            slot: 2,
            value: fake_module_exec as *mut c_void,
        },
        PyModuleDef_Slot {
            slot: 3,
            value: 2usize as *mut c_void,
        },
        PyModuleDef_Slot {
            slot: 4,
            value: 1usize as *mut c_void,
        },
        PyModuleDef_Slot {
            slot: 0,
            value: ptr::null_mut(),
        },
    ];
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
        m_name: c"moduledef_metadata_slots_module".as_ptr(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: ptr::null_mut(),
        m_slots: slots.as_mut_ptr(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };

    let module = unsafe {
        molt_cpython_abi::api::modules::PyModule_FromDefAndSpec2(&mut def, ptr::null_mut(), 0)
    };

    assert!(!module.is_null());
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module) };
}

#[test]
fn test_module_from_def_and_spec_registers_capi_state_once_before_exec() {
    let _guard = FAKE_MODULE_EXEC_LOCK.lock().unwrap();
    init();
    MODULE_EXEC_CALLED.store(0, Ordering::Relaxed);
    MODULE_EXEC_STATE_BYTE.store(0, Ordering::Relaxed);
    let mut slots = [
        PyModuleDef_Slot {
            slot: 2,
            value: fake_module_exec_mutates_state as *mut c_void,
        },
        PyModuleDef_Slot {
            slot: 0,
            value: ptr::null_mut(),
        },
    ];
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
        m_name: c"moduledef_state_exec_module".as_ptr(),
        m_doc: ptr::null(),
        m_size: 4,
        m_methods: ptr::null_mut(),
        m_slots: slots.as_mut_ptr(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };

    let module = unsafe {
        molt_cpython_abi::api::modules::PyModule_FromDefAndSpec2(&mut def, ptr::null_mut(), 0)
    };

    assert!(!module.is_null());
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 1);
    assert_eq!(MODULE_EXEC_STATE_BYTE.load(Ordering::Relaxed), 77);
    let state = unsafe { molt_cpython_abi::api::modules::PyModule_GetState(module) };
    assert!(!state.is_null());
    assert_eq!(unsafe { *(state as *mut u8) }, 77);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module) };
}

#[test]
fn test_module_from_def_and_spec_exec_failure_sets_error_message() {
    let _guard = FAKE_MODULE_EXEC_LOCK.lock().unwrap();
    init();
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    MODULE_EXEC_CALLED.store(0, Ordering::Relaxed);
    let mut slots = [
        PyModuleDef_Slot {
            slot: 2,
            value: fake_module_exec_failure as *mut c_void,
        },
        PyModuleDef_Slot {
            slot: 0,
            value: ptr::null_mut(),
        },
    ];
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
        m_name: c"moduledef_exec_failure_module".as_ptr(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: ptr::null_mut(),
        m_slots: slots.as_mut_ptr(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };

    let module = unsafe {
        molt_cpython_abi::api::modules::PyModule_FromDefAndSpec2(&mut def, ptr::null_mut(), 0)
    };

    assert!(module.is_null());
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 1);
    let message = molt_cpython_abi::api::errors::take_current_error_message()
        .expect("exec failure must enter CPython ABI error state");
    assert!(message.contains("Py_mod_exec slot returned non-zero"));
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
fn test_module_create2_registers_capi_state_and_pystate_registry_roundtrip() {
    init();
    let def = Box::leak(Box::new(PyModuleDef {
        m_base: PyModuleDef_Base {
            ob_base: PyObject {
                ob_refcnt: 1,
                ob_type: ptr::null_mut(),
            },
            m_init: None,
            m_index: 0,
            m_copy: ptr::null_mut(),
        },
        m_name: c"statefulmod".as_ptr(),
        m_doc: ptr::null(),
        m_size: 16,
        m_methods: ptr::null_mut(),
        m_slots: ptr::null_mut(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    }));

    let m = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(def, 1013) };
    assert!(!m.is_null());

    let state = unsafe { molt_cpython_abi::api::modules::PyModule_GetState(m) };
    assert!(!state.is_null());
    unsafe {
        assert_eq!(*(state as *mut u8).add(0), 0);
        *(state as *mut u8).add(0) = 42;
        assert_eq!(
            *(molt_cpython_abi::api::modules::PyModule_GetState(m) as *mut u8),
            42
        );
    }

    assert!(unsafe { molt_cpython_abi::api::modules::PyState_FindModule(def) }.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyState_AddModule(m, def) },
        0
    );
    let found = unsafe { molt_cpython_abi::api::modules::PyState_FindModule(def) };
    assert!(!found.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(found) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyState_RemoveModule(def) },
        0
    );
    assert!(unsafe { molt_cpython_abi::api::modules::PyState_FindModule(def) }.is_null());

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

unsafe extern "C" fn fake_c_method(_self: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

#[test]
fn test_module_create2_fails_closed_when_c_function_registration_fails() {
    init();
    let mut methods = [
        PyMethodDef {
            ml_name: c"reject".as_ptr(),
            ml_meth: Some(fake_c_method),
            ml_flags: METH_VARARGS,
            ml_doc: ptr::null(),
        },
        PyMethodDef {
            ml_name: ptr::null(),
            ml_meth: None,
            ml_flags: 0,
            ml_doc: ptr::null(),
        },
    ];
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
        m_name: c"rejectmod".as_ptr(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: methods.as_mut_ptr(),
        m_slots: ptr::null_mut(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(&mut def, 1013) };
    assert!(m.is_null());
    let message = molt_cpython_abi::api::errors::take_current_error_message()
        .expect("method registration failure must enter CPython ABI error state");
    assert!(message.contains("runtime rejected method"));
}

#[test]
fn test_module_create2_fails_closed_when_c_function_attr_publication_fails() {
    init();
    let mut methods = [
        PyMethodDef {
            ml_name: c"reject_attr".as_ptr(),
            ml_meth: Some(fake_c_method),
            ml_flags: METH_VARARGS,
            ml_doc: ptr::null(),
        },
        PyMethodDef {
            ml_name: ptr::null(),
            ml_meth: None,
            ml_flags: 0,
            ml_doc: ptr::null(),
        },
    ];
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
        m_name: c"rejectattrmod".as_ptr(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: methods.as_mut_ptr(),
        m_slots: ptr::null_mut(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(&mut def, 1013) };
    assert!(m.is_null());
    let message = molt_cpython_abi::api::errors::take_current_error_message()
        .expect("method publication failure must enter CPython ABI error state");
    assert!(message.contains("failed to register method"));
}
