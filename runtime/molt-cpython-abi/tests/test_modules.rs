//! Tests for PyModule_New, PyModule_GetDict, PyModule_Create2,
//! PyModule_AddObject, PyModule_AddIntConstant, PyModule_AddStringConstant,
//! PyModuleDef_Init.

#![allow(non_snake_case)]

mod support;

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::{
    BorrowedHandleResult, MoltBufferView, OwnedHandleResult, RuntimeHooks,
};
use molt_lang_obj_model::MoltObject;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

// ─── Test hook implementations ───────────────────────────────────────────────
//
// `molt-lang-cpython-abi` deliberately does not depend on `molt-lang-runtime`
// (avoids a circular dep), so integration tests in this crate cannot pull in
// the real runtime's hook implementations.  Instead we install a minimal
// hook vtable whose allocators all use the shared fixture owner registry.
// Module C-API state, call dispatch and rejection probes remain local
// observations; strings, dictionaries, modules and foreign edges have one
// allocator and terminal-retirement authority.
//
// The real runtime overrides this in production via
// `molt_cpython_abi_register_hooks`.

static FAKE_BUFFER_RELEASES: AtomicU64 = AtomicU64::new(0);
static MODULE_EXEC_CALLED: AtomicU64 = AtomicU64::new(0);
static MODULE_EXEC_STATE_BYTE: AtomicU64 = AtomicU64::new(0);
static MODULE_CREATE_CALLED: AtomicU64 = AtomicU64::new(0);
/// Serializes EVERY test in this binary. These tests exercise the ABI against
/// the process-global `GLOBAL_BRIDGE` (bidirectional handle<->PyObject proxy
/// table with non-atomically-refcounted, value-deduped proxies), the global
/// runtime-hook vtable, and the fake buffer-release / module-registry fixtures
/// below. In production every C-extension ABI call is GIL-serialized, so that
/// shared mutable state is only ever touched single-threaded; under
/// `cargo test`'s parallel threads it is not, and concurrent proxy churn
/// intermittently evicts a still-live handle (e.g. `PyState_FindModule` then
/// resolves a module to a fresh proxy pointer != the created one) or drops a
/// runtime buffer release, breaking the memoryview lifetime assertion. Holding
/// this lock for each test's duration restores the single-threaded invariant —
/// the same `TEST_LOCK` convention used across this crate's integration tests.
/// It subsumes the former per-fixture `FAKE_BUFFER_LOCK` / `FAKE_MODULE_EXEC_LOCK`
/// gates (which only serialized their own tests, leaving the shared-bridge race
/// open) and is acquired poison-tolerantly so one test's failure never cascades.
static TEST_LOCK: Mutex<()> = Mutex::new(());
static FAKE_BUFFER: [u8; 4] = [1, 2, 3, 4];
static FAKE_MODULE_STATE: LazyLock<Mutex<FakeModuleState>> =
    LazyLock::new(|| Mutex::new(FakeModuleState::default()));
static FAKE_CLASS_OVERRIDES: LazyLock<Mutex<HashMap<u64, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static FAKE_CALLABLE_BITS: LazyLock<Mutex<HashSet<u64>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static FAKE_CALL_ENABLED: AtomicBool = AtomicBool::new(false);
static FAKE_CALLS: AtomicU64 = AtomicU64::new(0);
static FAKE_LAST_CALLED: AtomicU64 = AtomicU64::new(0);
static SEMANTIC_CALL_SLOT_CALLS: AtomicU64 = AtomicU64::new(0);
static CROSSING_TEST: AtomicBool = AtomicBool::new(false);
static CROSSING_FAIL: AtomicBool = AtomicBool::new(false);
static CROSSING_CLEANUP_ERROR: AtomicBool = AtomicBool::new(false);
static CROSSING_SET_CALLS: AtomicU64 = AtomicU64::new(0);
static CROSSING_DEALLOCS: AtomicU64 = AtomicU64::new(0);
static CROSSING_ERROR_VALUE: AtomicUsize = AtomicUsize::new(0);
static CROSSING_FOREIGN: LazyLock<Mutex<HashMap<u64, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
// Observations only; each dictionary is the sole owner of its entries.
static CROSSING_STORED: Mutex<Vec<(u64, Vec<u8>, u64)>> = Mutex::new(Vec::new());

// Borrowed, exact C tuple layout for call-path tests. The fake runtime tuple
// hooks intentionally do not model tuple contents, so these stack values
// exercise the C tuple ingress without changing that fixture's semantics.
#[repr(C)]
struct CallTuple<const N: usize> {
    ob_base: PyVarObject,
    ob_item: [*mut PyObject; N],
}

impl<const N: usize> CallTuple<N> {
    fn new(items: [*mut PyObject; N]) -> Self {
        Self {
            ob_base: PyVarObject {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: &raw mut PyTuple_Type,
                },
                ob_size: N as isize,
            },
            ob_item: items,
        }
    }
}

#[derive(Default)]
struct FakeModuleState {
    capi_by_module: HashMap<u64, FakeModuleCapi>,
    by_def: HashMap<usize, u64>,
}

#[derive(Default)]
struct FakeModuleCapi {
    definition: usize,
    state: Option<Box<[u8]>>,
    state_size: usize,
    entered: bool,
}

unsafe extern "C" fn fake_alloc_bytes(_data: *const u8, _len: usize) -> u64 {
    support::fake_runtime::fresh_handle()
}
unsafe extern "C" fn fake_int_from_i64(_value: i64) -> u64 {
    support::fake_runtime::fresh_handle()
}
unsafe extern "C" fn fake_int_from_u64(_value: u64) -> u64 {
    support::fake_runtime::fresh_handle()
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
unsafe extern "C" fn fake_int_as_u64_mask(
    _bits: u64,
    _width: u32,
    _out: *mut u64,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn fake_int_from_bytes(
    _data: *const u8,
    _len: usize,
    _little_endian: std::os::raw::c_int,
    _signed: std::os::raw::c_int,
) -> u64 {
    support::fake_runtime::fresh_handle()
}

unsafe extern "C" fn fake_int_from_digits(
    _digits: *const u8,
    _len: usize,
    _base: u32,
    _negative: std::os::raw::c_int,
) -> u64 {
    0
}

unsafe extern "C" fn fake_int_from_f64_trunc(value: f64) -> u64 {
    unsafe { fake_int_from_i64(value.trunc() as i64) }
}

unsafe extern "C" fn fake_int_sign(bits: u64) -> i32 {
    unsafe { fake_int_as_i64(bits) }.signum() as i32
}

unsafe extern "C" fn fake_int_signed_byte_width(bits: u64, out: *mut usize) -> i32 {
    let value = unsafe { fake_int_as_i64(bits) };
    unsafe {
        *out = ((65
            - if value >= 0 {
                value.leading_zeros()
            } else {
                (!value).leading_zeros()
            }) as usize)
            .div_ceil(8)
    };
    0
}
unsafe extern "C" fn fake_int_to_bytes(
    _bits: u64,
    _data: *mut u8,
    _len: usize,
    _little_endian: std::os::raw::c_int,
    _signed: std::os::raw::c_int,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn fake_int_num_bits(_bits: u64, _out: *mut usize) -> std::os::raw::c_int {
    -1
}

unsafe extern "C" fn fake_int_max_str_digits() -> usize {
    4300
}

unsafe extern "C" fn fake_complex_parts(_bits: u64, _real: *mut f64, _imag: *mut f64) -> i32 {
    -1
}
unsafe extern "C" fn fake_alloc_list() -> u64 {
    support::fake_runtime::fresh_handle()
}
unsafe extern "C" fn fake_alloc_list_presized(_len: usize) -> u64 {
    support::fake_runtime::fresh_handle()
}
unsafe extern "C" fn fake_list_append(
    _list_bits: u64,
    _item_bits: u64,
    _item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> i32 {
    0
}
unsafe extern "C" fn fake_list_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn fake_list_item(_bits: u64, _i: usize) -> BorrowedHandleResult {
    BorrowedHandleResult::missing()
}
unsafe extern "C" fn fake_list_set(
    _list_bits: u64,
    _i: usize,
    _val_bits: u64,
) -> OwnedHandleResult {
    OwnedHandleResult::ok(MoltObject::none().bits())
}
unsafe extern "C" fn fake_list_insert(
    _list_bits: u64,
    _where_: isize,
    _item_bits: u64,
    _item_ptr: *mut molt_cpython_abi::abi_types::PyObject,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn fake_list_sort(_list_bits: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn fake_list_reverse(_list_bits: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn fake_list_set_slice(
    _list_bits: u64,
    _ilow: isize,
    _ihigh: isize,
    _replacement: *const u64,
    _replacement_len: usize,
    _future_pointers: *const *mut molt_cpython_abi::abi_types::PyObject,
    _future_len: usize,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn fake_alloc_tuple(_arity: usize) -> u64 {
    support::fake_runtime::fresh_handle()
}
unsafe extern "C" fn fake_tuple_set(
    _bits: u64,
    _i: usize,
    _value: u64,
    _exact_pointer: *mut PyObject,
) -> OwnedHandleResult {
    OwnedHandleResult::ok(MoltObject::none().bits())
}
unsafe extern "C" fn fake_tuple_len(_bits: u64) -> usize {
    0
}
unsafe extern "C" fn fake_tuple_item(_bits: u64, _i: usize) -> BorrowedHandleResult {
    BorrowedHandleResult::missing()
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
unsafe extern "C" fn fake_object_get_attr(
    obj: u64,
    name: u64,
    _access: molt_cpython_abi::hooks::AttributeAccess,
    _dictionary: *const u64,
    _suppress: bool,
) -> OwnedHandleResult {
    let molt_cpython_abi::hooks::DecodedHandleResult::Ok(dict) =
        (unsafe { support::fake_runtime::module_get_dict(obj) }).decode()
    else {
        return OwnedHandleResult::error();
    };
    match unsafe {
        support::fake_runtime::dict_get(
            dict,
            name,
            molt_cpython_abi::hooks::DictHashSource::Compute,
            0,
        )
    }
    .decode()
    {
        molt_cpython_abi::hooks::DecodedHandleResult::Ok(value) => {
            unsafe { support::fake_runtime::inc_ref(value) };
            OwnedHandleResult::ok(value)
        }
        _ => OwnedHandleResult::error(),
    }
}
unsafe extern "C" fn fake_module_exec_begin(module: u64, _def: usize) -> i32 {
    let mut guard = FAKE_MODULE_STATE.lock().unwrap();
    let entry = guard.capi_by_module.get_mut(&module).unwrap();
    let prior = entry.entered;
    entry.entered = true;
    entry
        .state
        .get_or_insert_with(|| vec![0; entry.state_size].into_boxed_slice());
    i32::from(prior)
}
unsafe extern "C" fn fake_object_set_attr(
    _obj: u64,
    _name: u64,
    _value: u64,
    _delete: bool,
    _access: molt_cpython_abi::hooks::AttributeMutation,
) -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn fake_object_format(_obj: u64, _spec: u64) -> OwnedHandleResult {
    OwnedHandleResult::ok(support::fake_runtime::fresh_handle())
}
unsafe extern "C" fn fake_sys_get_object_borrowed(
    _data: *const u8,
    _len: usize,
    _policy: molt_cpython_abi::hooks::SysLookupPolicy,
) -> BorrowedHandleResult {
    BorrowedHandleResult::missing()
}
unsafe extern "C" fn fake_try_mark_abi_view(
    _bits: u64,
    _present: std::os::raw::c_int,
) -> std::os::raw::c_int {
    1
}
unsafe extern "C" fn fake_import_add_module_borrowed(
    _data: *const u8,
    _len: usize,
) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn fake_eval_get_builtins_borrowed() -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn fake_module_set_attr(
    module: u64,
    data: *const u8,
    len: usize,
    value: u64,
) -> std::os::raw::c_int {
    if CROSSING_TEST.load(Ordering::Relaxed) {
        CROSSING_SET_CALLS.fetch_add(1, Ordering::Relaxed);
    }

    if !data.is_null() {
        let name = unsafe { std::slice::from_raw_parts(data, len) };
        if name == b"reject_attr" {
            return -1;
        }
        if name == b"reject_attr_with_error" {
            unsafe {
                molt_cpython_abi::api::errors::PyErr_SetString(
                    (&raw mut PyExc_ValueError).cast::<PyObject>(),
                    c"method publication detail".as_ptr(),
                );
            }
            if CROSSING_TEST.load(Ordering::Relaxed) {
                let error = molt_cpython_abi::api::errors::take_current_error().unwrap();
                CROSSING_ERROR_VALUE.store(error.value.addr(), Ordering::Relaxed);
                molt_cpython_abi::api::errors::restore_current_error_exact(error);
            }
            return -1;
        }
    }
    let molt_cpython_abi::hooks::DecodedHandleResult::Ok(dict) =
        (unsafe { support::fake_runtime::module_get_dict(module) }).decode()
    else {
        return -1;
    };
    let key = unsafe { support::fake_runtime::alloc_str(data, len) };
    let status =
        unsafe { support::fake_runtime::dict_mutate(dict, key, value, 0, None, ptr::null_mut()) };
    molt_cpython_abi::api::errors::with_preserved_error(|| unsafe {
        support::fake_runtime::dec_ref(key)
    });
    if status == 0 && CROSSING_TEST.load(Ordering::Relaxed) {
        let name = if data.is_null() {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
        };
        CROSSING_STORED.lock().unwrap().push((dict, name, value));
    }
    status
}
unsafe extern "C" fn fake_module_capi_register(
    module_bits: u64,
    module_def_ptr: usize,
    module_state_size: u64,
    defer_state: bool,
    _callbacks: molt_cpython_abi::hooks::ModuleGcCallbacks,
) -> std::os::raw::c_int {
    let Ok(size) = usize::try_from(module_state_size) else {
        return -1;
    };
    let state = if defer_state || size == 0 {
        None
    } else {
        Some(vec![0; size].into_boxed_slice())
    };
    let mut guard = FAKE_MODULE_STATE.lock().unwrap();
    if guard.capi_by_module.contains_key(&module_bits) {
        return -1;
    }
    guard.capi_by_module.insert(
        module_bits,
        FakeModuleCapi {
            definition: module_def_ptr,
            state,
            state_size: size,
            entered: false,
        },
    );
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
unsafe extern "C" fn fake_module_capi_get_def(module_bits: u64) -> usize {
    FAKE_MODULE_STATE
        .lock()
        .unwrap()
        .capi_by_module
        .get(&module_bits)
        .map_or(0, |entry| entry.definition)
}
unsafe extern "C" fn fake_module_state_add(
    module_bits: u64,
    module_def_ptr: usize,
) -> std::os::raw::c_int {
    if module_def_ptr == 0 {
        return -1;
    }
    let replaced = FAKE_MODULE_STATE
        .lock()
        .unwrap()
        .by_def
        .insert(module_def_ptr, module_bits);
    if replaced != Some(module_bits) {
        unsafe { support::fake_runtime::inc_ref(module_bits) };
        if let Some(old_bits) = replaced {
            unsafe { support::fake_runtime::dec_ref(old_bits) };
        }
    }
    0
}
unsafe extern "C" fn fake_module_state_find(module_def_ptr: usize) -> BorrowedHandleResult {
    match FAKE_MODULE_STATE
        .lock()
        .unwrap()
        .by_def
        .get(&module_def_ptr)
        .copied()
    {
        Some(bits) => BorrowedHandleResult::ok(bits),
        None => BorrowedHandleResult::missing(),
    }
}
unsafe extern "C" fn fake_module_state_remove(module_def_ptr: usize) -> std::os::raw::c_int {
    if let Some(bits) = FAKE_MODULE_STATE
        .lock()
        .unwrap()
        .by_def
        .remove(&module_def_ptr)
    {
        unsafe { support::fake_runtime::dec_ref(bits) };
        0
    } else {
        -1
    }
}
unsafe extern "C" fn fake_register_c_function(
    _meth: u64,
    _flags: std::os::raw::c_int,
    _self_bits: u64,
    _self_is_null: bool,
    _defining_class_bits: u64,
    data: *const u8,
    len: usize,
) -> u64 {
    if !data.is_null() {
        let name = unsafe { std::slice::from_raw_parts(data, len) };
        if name == b"reject" {
            return 0;
        }
        if name == b"reject_with_error" {
            unsafe {
                molt_cpython_abi::api::errors::PyErr_SetString(
                    (&raw mut PyExc_MemoryError).cast::<PyObject>(),
                    c"callable allocation detail".as_ptr(),
                );
            }
            return 0;
        }
    }
    unsafe {
        support::fake_runtime::register_c_function(
            _meth,
            _flags,
            _self_bits,
            _self_is_null,
            _defining_class_bits,
            data,
            len,
        )
    }
}

unsafe extern "C" fn fake_import_module(_data: *const u8, _len: usize) -> u64 {
    0
}

unsafe extern "C" fn fake_exception_pending() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn fake_report_unraisable(
    _context_bits: u64,
    _type_bits: u64,
    _value_bits: u64,
    _traceback_bits: u64,
    _message: *const u8,
    _message_len: usize,
    _err_msg: *const u8,
    _err_msg_len: usize,
    _has_err_msg: std::os::raw::c_int,
) {
}
unsafe extern "C" fn fake_exception_set_field(
    _exception_bits: u64,
    _field: u32,
    _value_bits: u64,
    _has_value: std::os::raw::c_int,
) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn fake_exception_get_field(
    _exception_bits: u64,
    _field: u32,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn fake_runtime_class_borrowed(value_bits: u64) -> BorrowedHandleResult {
    if let Some(class_bits) = FAKE_CLASS_OVERRIDES
        .lock()
        .unwrap()
        .get(&value_bits)
        .copied()
    {
        return BorrowedHandleResult::ok(class_bits);
    }
    unsafe { support::fake_runtime::runtime_class_borrowed(value_bits) }
}
unsafe extern "C" fn fake_take_pending_exception(
    _actual_class_bits: *mut u64,
    _traceback_bits: *mut u64,
) -> OwnedHandleResult {
    OwnedHandleResult::error()
}

unsafe extern "C" fn fake_gil_ensure() -> std::os::raw::c_int {
    0
}
unsafe extern "C" fn fake_gil_leave(_state: std::os::raw::c_int) {}
unsafe extern "C" fn fake_gil_release() {}
unsafe extern "C" fn fake_gil_restore() {}
unsafe extern "C" fn fake_gil_check() -> std::os::raw::c_int {
    1
}
unsafe extern "C" fn fake_runtime_is_initialized() -> std::os::raw::c_int {
    1
}
unsafe extern "C" fn fake_thread_state_drop_enter() -> u64 {
    1
}
unsafe extern "C" fn fake_thread_state_drop_leave(_token: u64) {}
unsafe extern "C" fn fake_attached_runtime_context() -> u32 {
    molt_cpython_abi::hooks::AttachedRuntimeContextKind::NativeGil as u32
}
unsafe extern "C" fn fake_pending_call_error(_reason: u32) {}
unsafe extern "C" fn fake_clear_pending_exception() {}

unsafe extern "C" fn fake_type_dict_borrowed(_type: u64) -> BorrowedHandleResult {
    BorrowedHandleResult::error()
}
unsafe extern "C" fn fake_type_lookup_borrowed(
    _type: u64,
    _name: u64,
    _mro: u8,
) -> BorrowedHandleResult {
    BorrowedHandleResult::missing()
}

const TEST_HOOKS: RuntimeHooks = RuntimeHooks {
    abi_magic: molt_cpython_abi::hooks::RUNTIME_HOOKS_ABI_MAGIC,
    abi_version: molt_cpython_abi::hooks::RUNTIME_HOOKS_ABI_VERSION,
    struct_size: std::mem::size_of::<RuntimeHooks>() as u32,
    gil_ensure: fake_gil_ensure,
    gil_leave: fake_gil_leave,
    gil_release: fake_gil_release,
    gil_restore: fake_gil_restore,
    gil_check: fake_gil_check,
    runtime_is_initialized: fake_runtime_is_initialized,
    thread_state_drop_enter: fake_thread_state_drop_enter,
    thread_state_drop_leave: fake_thread_state_drop_leave,
    attached_runtime_context: fake_attached_runtime_context,
    pending_call_error: fake_pending_call_error,
    alloc_str: support::fake_runtime::alloc_str,
    alloc_bytes: fake_alloc_bytes,
    int_from_i64: fake_int_from_i64,
    int_from_u64: fake_int_from_u64,
    int_as_i64: fake_int_as_i64,
    int_as_i64_checked: fake_int_as_i64_checked,
    int_as_u64_checked: fake_int_as_u64_checked,
    int_as_u64_mask: fake_int_as_u64_mask,
    int_from_digits: fake_int_from_digits,
    int_from_f64_trunc: fake_int_from_f64_trunc,
    int_sign: fake_int_sign,
    int_signed_byte_width: fake_int_signed_byte_width,
    int_from_bytes: fake_int_from_bytes,
    int_to_bytes: fake_int_to_bytes,
    int_num_bits: fake_int_num_bits,
    int_max_str_digits: fake_int_max_str_digits,
    complex_parts: fake_complex_parts,
    alloc_list: fake_alloc_list,
    alloc_list_presized: fake_alloc_list_presized,
    list_append: fake_list_append,
    list_len: fake_list_len,
    list_item: fake_list_item,
    list_set: fake_list_set,
    list_insert: fake_list_insert,
    list_sort: fake_list_sort,
    list_reverse: fake_list_reverse,
    list_set_slice: fake_list_set_slice,
    alloc_tuple: fake_alloc_tuple,
    tuple_set: fake_tuple_set,
    tuple_len: fake_tuple_len,
    tuple_item: fake_tuple_item,
    alloc_dict: support::fake_runtime::alloc_dict,
    dict_resolve: support::fake_runtime::dict_resolve,
    dict_mutate: support::fake_runtime::dict_mutate,
    dict_get: support::fake_runtime::dict_get,
    dict_pop: support::fake_runtime::dict_pop,
    dict_len: support::fake_runtime::dict_len,
    dict_entry: support::fake_runtime::dict_entry,
    str_data: support::fake_runtime::str_data,
    bytes_data: fake_bytes_data,
    buffer_acquire: fake_buffer_acquire,
    buffer_release: fake_buffer_release,
    object_get_attr: fake_object_get_attr,
    type_dict_borrowed: fake_type_dict_borrowed,
    type_lookup_borrowed: fake_type_lookup_borrowed,
    object_set_attr: fake_object_set_attr,
    object_format: fake_object_format,
    object_str: support::fake_runtime::object_str,
    object_repr: support::fake_runtime::object_repr,
    sys_get_object_borrowed: fake_sys_get_object_borrowed,
    eval_get_builtins_borrowed: fake_eval_get_builtins_borrowed,
    classify_heap: support::fake_runtime::classify_heap,
    inc_ref: support::fake_runtime::inc_ref,
    dec_ref: support::fake_runtime::dec_ref,
    ref_count: support::fake_runtime::ref_count,
    try_mark_abi_view: fake_try_mark_abi_view,
    alloc_module: support::fake_runtime::alloc_module,
    module_get_dict_borrowed: support::fake_runtime::module_get_dict,
    import_add_module_borrowed: fake_import_add_module_borrowed,
    module_set_attr: fake_module_set_attr,
    module_capi_register: fake_module_capi_register,
    module_capi_get_state: fake_module_capi_get_state,
    module_capi_get_def: fake_module_capi_get_def,
    module_state_add: fake_module_state_add,
    module_state_find: fake_module_state_find,
    module_state_remove: fake_module_state_remove,
    module_exec_begin: fake_module_exec_begin,
    register_c_function: fake_register_c_function,
    import_module: fake_import_module,
    exception_pending: fake_exception_pending,
    number_binary_op: fake_number_binary_op,
    number_unary_op: fake_number_unary_op,
    number_power: fake_number_power,
    dict_op: support::fake_runtime::dict_op,
    set_op: fake_set_op,
    set_new: fake_set_new,
    set_size: fake_set_size,
    set_contains: fake_set_contains,
    set_add: fake_set_add,
    set_discard: fake_set_discard,
    object_dir: fake_object_dir,
    object_call: fake_object_call,
    object_is_callable: fake_object_is_callable,
    foreign_new: fake_foreign_new,
    report_unraisable: fake_report_unraisable,
    exception_set_field: fake_exception_set_field,
    exception_get_field: fake_exception_get_field,
    runtime_class_borrowed: fake_runtime_class_borrowed,
    take_pending_exception: fake_take_pending_exception,
    clear_pending_exception: fake_clear_pending_exception,
    with_preserved_pending_exception: molt_cpython_abi::hooks::STUB_HOOKS
        .with_preserved_pending_exception,
    ..molt_cpython_abi::hooks::STUB_HOOKS
};

unsafe extern "C" fn fake_object_call(
    callable: u64,
    _args: u64,
    _kwargs: u64,
) -> OwnedHandleResult {
    if FAKE_CALL_ENABLED.load(Ordering::Relaxed) {
        FAKE_LAST_CALLED.store(callable, Ordering::Relaxed);
        FAKE_CALLS.fetch_add(1, Ordering::Relaxed);
        return OwnedHandleResult::ok(support::fake_runtime::fresh_handle());
    }
    OwnedHandleResult::error()
}

unsafe extern "C" fn fake_object_is_callable(bits: u64) -> std::os::raw::c_int {
    std::os::raw::c_int::from(FAKE_CALLABLE_BITS.lock().unwrap().contains(&bits))
}

unsafe extern "C" fn semantic_call_slot(
    _callable: *mut PyObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    SEMANTIC_CALL_SLOT_CALLS.fetch_add(1, Ordering::Relaxed);
    ptr::null_mut()
}
fn observe_foreign_retirement(bits: u64) {
    assert!(CROSSING_FOREIGN.lock().unwrap().remove(&bits).is_some());
    if CROSSING_CLEANUP_ERROR.load(Ordering::Relaxed) {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_KeyError).cast(),
                c"crossing cleanup error".as_ptr(),
            );
        }
    }
}
unsafe extern "C" fn fake_foreign_new(c_ptr: usize) -> u64 {
    if CROSSING_TEST.load(Ordering::Relaxed) && CROSSING_FAIL.load(Ordering::Relaxed) {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_MemoryError).cast(),
                c"foreign allocation rejected".as_ptr(),
            )
        };
        return 0;
    }
    let bits = unsafe { support::fake_runtime::foreign_new(c_ptr) };
    if CROSSING_TEST.load(Ordering::Relaxed) {
        CROSSING_FOREIGN.lock().unwrap().insert(bits, c_ptr);
        support::fake_runtime::observe_retirement(bits, observe_foreign_retirement);
    }
    bits
}
unsafe extern "C" fn fake_number_binary_op(_op: u32, _a: u64, _b: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn fake_number_unary_op(_op: u32, _a: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn fake_number_power(_a: u64, _b: u64, _mod_bits: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn fake_set_new(_iterable: BorrowedHandleResult, _frozen: bool) -> u64 {
    0
}
unsafe extern "C" fn fake_set_size(_set: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn fake_set_contains(_set: u64, _key: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn fake_set_add(_set: u64, _key: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn fake_set_discard(_set: u64, _key: u64) -> std::os::raw::c_int {
    -1
}
unsafe extern "C" fn fake_set_op(_op: u32, _set: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}
unsafe extern "C" fn fake_object_dir(_obj: u64) -> OwnedHandleResult {
    OwnedHandleResult::error()
}

/// Acquire the binary-wide serialization guard (poison-tolerant, so one test's
/// failure never cascades into the rest) and run the idempotent ABI + hook init.
/// Every test binds the returned guard for its whole body — see `TEST_LOCK`.
/// The returned `MutexGuard` is itself `#[must_use]`, so a test that drops it
/// early (a bare `init();`) is caught at compile time.
fn init() -> MutexGuard<'static, ()> {
    let guard = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    support::prepare_abi_test_thread(TEST_HOOKS);
    guard
}

#[test]
fn generic_managed_instance_reports_runtime_class_without_layout_stamping() {
    let _guard = init();
    let class_bits = support::fake_runtime::fresh_handle();
    let mut class: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    class.ob_base.ob_base.ob_refcnt = 1;
    class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    class.tp_name = c"fixture.Custom".as_ptr();
    class.tp_base = &raw mut PyBaseObject_Type;
    let class_ptr = &mut *class as *mut PyTypeObject;
    unsafe {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .bind_static_pyobj_to_runtime_handle(class_ptr.cast(), class_bits, true)
            .expect("bind a custom runtime class to its existing ABI Type view");
    }
    let instance_bits = support::fake_runtime::fresh_handle();
    FAKE_CLASS_OVERRIDES
        .lock()
        .unwrap()
        .insert(instance_bits, class_bits);
    let instance =
        unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(instance_bits) };
    assert!(!instance.is_null());
    assert_eq!(unsafe { (*instance).ob_type }, &raw mut MoltManaged_Type);
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::_Py_TYPE(instance) },
        class_ptr
    );
    let initial_refs = class.ob_base.ob_base.ob_refcnt;
    let owned_class = unsafe { molt_cpython_abi::api::typeobj::PyObject_Type(instance) };
    assert_eq!(owned_class, class_ptr.cast());
    assert_eq!(class.ob_base.ob_base.ob_refcnt, initial_refs + 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(owned_class) };
    assert_eq!(class.ob_base.ob_base.ob_refcnt, initial_refs);
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::_Py_TYPE(instance) },
        class_ptr
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(instance) };
    FAKE_CLASS_OVERRIDES.lock().unwrap().remove(&instance_bits);
    assert!(unsafe {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .unbind_static_pyobj_from_runtime_handle(class_ptr.cast(), class_bits)
    });
    unsafe { support::fake_runtime::dec_ref(class_bits) };
}

#[test]
fn managed_call_dispatch_ignores_semantic_class_slots_but_type_reports_that_class() {
    let _guard = init();
    let class_bits = support::fake_runtime::fresh_handle();
    let mut class: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    class.ob_base.ob_base.ob_refcnt = 1;
    class.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    class.tp_name = c"fixture.Callable".as_ptr();
    class.tp_call = Some(semantic_call_slot);
    let class_ptr = &mut *class as *mut PyTypeObject;
    unsafe {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .bind_static_pyobj_to_runtime_handle(class_ptr.cast(), class_bits, true)
            .expect("bind semantic class view");
    }
    let instance_bits = support::fake_runtime::fresh_handle();
    FAKE_CLASS_OVERRIDES
        .lock()
        .unwrap()
        .insert(instance_bits, class_bits);
    let instance =
        unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(instance_bits) };
    assert!(!instance.is_null());
    assert_eq!(unsafe { (*instance).ob_type }, &raw mut MoltManaged_Type);

    FAKE_CALLS.store(0, Ordering::Relaxed);
    SEMANTIC_CALL_SLOT_CALLS.store(0, Ordering::Relaxed);
    // A projected semantic class advertises tp_call, but the runtime oracle
    // still gets to say this particular managed instance is not callable.
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyCallable_Check(instance) },
        0
    );
    FAKE_CALL_ENABLED.store(true, Ordering::Relaxed);
    FAKE_CALLABLE_BITS.lock().unwrap().insert(instance_bits);
    unsafe {
        use molt_cpython_abi::api::{object, refcount, typeobj};
        assert_eq!(typeobj::PyCallable_Check(instance), 1);
        let direct = object::PyObject_Call(instance, ptr::null_mut(), ptr::null_mut());
        assert!(!direct.is_null());
        refcount::Py_DECREF(direct);

        let vector = object::PyObject_Vectorcall(instance, ptr::null_mut(), 0, ptr::null_mut());
        assert!(!vector.is_null());
        refcount::Py_DECREF(vector);

        let vector_dict =
            object::PyObject_VectorcallDict(instance, ptr::null_mut(), 0, ptr::null_mut());
        assert!(!vector_dict.is_null());
        refcount::Py_DECREF(vector_dict);

        // The direct metatype slot is public to C callers too. Even there a
        // managed view must not try to instantiate its semantic class layout.
        let slot = typeobj::molt_type_call(instance, ptr::null_mut(), ptr::null_mut());
        assert!(!slot.is_null());
        refcount::Py_DECREF(slot);

        let mut type_args = CallTuple::new([instance]);
        let reported = object::PyObject_Call(
            (&raw mut PyType_Type).cast(),
            (&raw mut type_args).cast(),
            ptr::null_mut(),
        );
        assert_eq!(reported, class_ptr.cast());
        refcount::Py_DECREF(reported);
        refcount::Py_DECREF(instance);
    }
    FAKE_CALLABLE_BITS.lock().unwrap().remove(&instance_bits);
    FAKE_CALL_ENABLED.store(false, Ordering::Relaxed);
    assert_eq!(FAKE_CALLS.load(Ordering::Relaxed), 4);
    assert_eq!(FAKE_LAST_CALLED.load(Ordering::Relaxed), instance_bits);
    assert_eq!(SEMANTIC_CALL_SLOT_CALLS.load(Ordering::Relaxed), 0);
    FAKE_CLASS_OVERRIDES.lock().unwrap().remove(&instance_bits);
    assert!(unsafe {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .unbind_static_pyobj_from_runtime_handle(class_ptr.cast(), class_bits)
    });
    unsafe { support::fake_runtime::dec_ref(class_bits) };
}

#[test]
fn bound_type_shell_uses_runtime_constructor_when_native_tp_new_is_absent() {
    let _guard = init();
    let _ = unsafe { support::fake_runtime::runtime_class_borrowed(MoltObject::none().bits()) };
    assert!(unsafe { (*(&raw mut PyType_Type)).tp_new.is_none() });
    FAKE_CALLS.store(0, Ordering::Relaxed);
    FAKE_CALL_ENABLED.store(true, Ordering::Relaxed);
    unsafe {
        use molt_cpython_abi::api::{object, refcount, typeobj};
        let mut args = CallTuple::new([(&raw mut Py_None).cast(); 3]);
        let constructed = object::PyObject_Call(
            (&raw mut PyType_Type).cast(),
            (&raw mut args).cast(),
            ptr::null_mut(),
        );
        assert!(!constructed.is_null());
        refcount::Py_DECREF(constructed);
        assert_eq!(FAKE_CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(
            FAKE_LAST_CALLED.load(Ordering::Relaxed),
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_handle_for_pyobj((&raw mut PyType_Type).cast())
                .unwrap()
                .bits()
        );

        // A direct invocation of the metatype slot has the same authority.
        let direct = typeobj::molt_type_call(
            (&raw mut PyType_Type).cast(),
            ptr::null_mut(),
            ptr::null_mut(),
        );
        assert!(direct.is_null());
        // `type()` still rejects the zero-argument shape before delegation.
        assert_eq!(FAKE_CALLS.load(Ordering::Relaxed), 1);
        molt_cpython_abi::api::errors::PyErr_Clear();
    }
    FAKE_CALL_ENABLED.store(false, Ordering::Relaxed);
}

#[test]
fn missing_managed_class_identity_stops_type_call_and_attribute_dispatch() {
    let _guard = init();
    let instance_bits = support::fake_runtime::fresh_handle();
    FAKE_CLASS_OVERRIDES
        .lock()
        .unwrap()
        .insert(instance_bits, 0);
    let instance =
        unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(instance_bits) };
    assert!(!instance.is_null());
    assert_eq!(unsafe { (*instance).ob_type }, &raw mut MoltManaged_Type);
    unsafe {
        use molt_cpython_abi::api::{errors, object, refcount, typeobj};
        assert!(typeobj::PyObject_Type(instance).is_null());
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        assert!(object::PyObject_Call(instance, ptr::null_mut(), ptr::null_mut()).is_null());
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        assert_eq!(typeobj::PyCallable_Check(instance), 0);
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        assert_eq!(
            object::PyObject_SetAttrString(instance, c"field".as_ptr(), instance),
            -1
        );
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        refcount::Py_DECREF(instance);
    }
    FAKE_CLASS_OVERRIDES.lock().unwrap().remove(&instance_bits);
}

#[test]
fn dict_set_item_anchors_key_and_value_proxies() {
    let _guard = init();
    let (recv, key, value) = unsafe {
        (
            molt_cpython_abi::api::mapping::PyDict_New(),
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .owned_handle_to_pyobj(support::fake_runtime::fresh_handle()),
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .owned_handle_to_pyobj(support::fake_runtime::fresh_handle()),
        )
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::mapping::PyDict_SetItem(recv, key, value) },
        0
    );
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(key);
        molt_cpython_abi::api::refcount::Py_DECREF(value);
    }
    assert!(
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(key)
            .is_some(),
        "key mapping severed by the extension's balancing DECREF"
    );
    assert!(
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(value)
            .is_some(),
        "value mapping severed by the extension's balancing DECREF"
    );
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(recv) };
}

#[test]
fn dict_set_item_gives_foreign_custody_to_key() {
    let _guard = init();
    let _fixture = CrossingFixture::new();
    let recv = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    let value = unsafe {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .owned_handle_to_pyobj(support::fake_runtime::fresh_handle())
    };
    let mut foreign_key = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut PyBaseObject_Type,
    };
    let key = &raw mut foreign_key;
    assert_eq!(
        unsafe { molt_cpython_abi::api::mapping::PyDict_SetItem(recv, key, value) },
        0,
        "foreign C-extension object rejected as a dict key"
    );
    let bits = unsafe {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_value_for_pyobj(key)
            .expect("foreign key must acquire stable bridge custody")
    };
    let round_trip = unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.owned_handle_to_pyobj(bits) };
    assert_eq!(round_trip, key, "foreign key wrapper lost pointer identity");
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(round_trip);
        molt_cpython_abi::api::refcount::Py_DECREF(value);
        molt_cpython_abi::api::refcount::Py_DECREF(recv);
    }
    assert_eq!(
        foreign_key.ob_refcnt, 1,
        "dictionary retirement releases its key custody"
    );
    assert!(CROSSING_FOREIGN.lock().unwrap().is_empty());
}

#[test]
fn test_getbuffer_uses_runtime_typed_descriptor() {
    let _guard = init();
    let obj = unsafe {
        molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .owned_handle_to_pyobj(support::fake_runtime::fresh_handle())
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
    let _guard = init();
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
    // CPython-exact FillInfo (Objects/abstract.c): allocation-free —
    // `internal` is NULL, `format` is the static "B", and shape/strides are
    // the self-referential field pointers `&view.len` / `&view.itemsize`.
    assert!(view.internal.is_null());
    assert!(!view.format.is_null());
    assert!(!view.shape.is_null());
    assert!(!view.strides.is_null());
    assert!(
        std::ptr::eq(view.shape.cast_const(), &raw const view.len),
        "FillInfo shape must be the CPython self-referential &view.len",
    );
    assert!(
        std::ptr::eq(view.strides.cast_const(), &raw const view.itemsize),
        "FillInfo strides must be the CPython self-referential &view.itemsize",
    );
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
    let _guard = init();
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

// ---------------------------------------------------------------------------
// PyModule_New
// ---------------------------------------------------------------------------

#[test]
fn test_module_new_non_null() {
    let _guard = init();
    unsafe {
        use molt_cpython_abi::api::{modules, refcount, strings};
        use molt_cpython_abi::hooks::DecodedHandleResult;
        let m = modules::PyModule_New(c"testmod".as_ptr());
        assert!(!m.is_null());
        let module_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(m)
            .unwrap()
            .bits();
        let DecodedHandleResult::Ok(dict) =
            support::fake_runtime::module_get_dict(module_bits).decode()
        else {
            panic!("module owns a dictionary");
        };
        let (mut name_key, mut name_value) = (0, 0);
        assert_eq!(
            support::fake_runtime::dict_entry(dict, 0, &mut name_key, &mut name_value),
            1
        );
        let text = strings::PyUnicode_FromString(c"projected string lifetime".as_ptr());
        assert!(!text.is_null());
        let text_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(text)
            .unwrap()
            .bits();
        assert!(support::fake_runtime::ref_count(text_bits) > 0);
        refcount::Py_DECREF(text);
        assert!(!support::fake_runtime::contains(text_bits));
        refcount::Py_DECREF(m);
        for bits in [module_bits, dict, name_key, name_value] {
            assert!(
                !support::fake_runtime::contains(bits),
                "module teardown retires the complete owned namespace"
            );
        }
    }
}

#[test]
fn test_module_new_null_name_returns_null() {
    let _guard = init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(ptr::null()) };
    assert!(m.is_null());
}

// ---------------------------------------------------------------------------
// PyModule_GetDict
// ---------------------------------------------------------------------------

#[test]
fn test_module_getdict_non_null() {
    let _guard = init();
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_New(c"mod".as_ptr()) };
    let d = unsafe { molt_cpython_abi::api::modules::PyModule_GetDict(m) };
    assert!(!d.is_null());
    let borrowed_refcnt = unsafe { (*d).ob_refcnt };
    for _ in 0..128 {
        let again = unsafe { molt_cpython_abi::api::modules::PyModule_GetDict(m) };
        assert_eq!(again, d);
        assert_eq!(
            unsafe { (*d).ob_refcnt },
            borrowed_refcnt,
            "PyModule_GetDict must not turn its borrowed hook result into a new C reference"
        );
    }
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

#[test]
fn test_module_getdict_null_returns_null() {
    let _guard = init();
    let d = unsafe { molt_cpython_abi::api::modules::PyModule_GetDict(ptr::null_mut()) };
    assert!(d.is_null());
}

// ---------------------------------------------------------------------------
// PyModule_AddObject
// ---------------------------------------------------------------------------

struct CrossingFixture;

impl CrossingFixture {
    fn new() -> Self {
        assert!(!CROSSING_TEST.swap(true, Ordering::Relaxed));
        assert!(CROSSING_FOREIGN.lock().unwrap().is_empty());
        assert!(CROSSING_STORED.lock().unwrap().is_empty());
        CROSSING_SET_CALLS.store(0, Ordering::Relaxed);
        CROSSING_DEALLOCS.store(0, Ordering::Relaxed);
        CROSSING_ERROR_VALUE.store(0, Ordering::Relaxed);
        Self
    }

    fn remove_values(&self) {
        let values = std::mem::take(&mut *CROSSING_STORED.lock().unwrap());
        for (dict, name, _) in values {
            // Name bytes are observations, not borrowed runtime identities:
            // an equal replacement key may already have retired its handle.
            if support::fake_runtime::contains(dict) {
                let key = unsafe { support::fake_runtime::alloc_str(name.as_ptr(), name.len()) };
                assert!(
                    unsafe {
                        support::fake_runtime::dict_mutate(dict, key, 0, 1, None, ptr::null_mut())
                    } >= 0
                );
                molt_cpython_abi::api::errors::with_preserved_error(|| unsafe {
                    support::fake_runtime::dec_ref(key)
                });
            }
        }
    }
}

impl Drop for CrossingFixture {
    fn drop(&mut self) {
        CROSSING_FAIL.store(false, Ordering::Relaxed);
        CROSSING_CLEANUP_ERROR.store(false, Ordering::Relaxed);
        self.remove_values();
        CROSSING_TEST.store(false, Ordering::Relaxed);
        unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    }
}

unsafe extern "C" fn crossing_dealloc(object: *mut PyObject) {
    CROSSING_DEALLOCS.fetch_add(1, Ordering::Relaxed);
    unsafe { drop(Box::from_raw(object)) };
}

fn crossing_object(typ: &mut PyTypeObject) -> *mut PyObject {
    typ.tp_dealloc = Some(crossing_dealloc);
    Box::into_raw(Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: typ,
    }))
}

#[test]
fn module_insertion_balances_foreign_temporary_and_exact_native_steal() {
    let _guard = init();
    let fixture = CrossingFixture::new();
    for steal in [false, true] {
        let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
        let value = crossing_object(&mut typ);
        unsafe {
            let module = molt_cpython_abi::api::modules::PyModule_New(c"custody".as_ptr());
            let result = if steal {
                molt_cpython_abi::api::modules::PyModule_AddObject(module, c"value".as_ptr(), value)
            } else {
                molt_cpython_abi::api::modules::PyModule_AddObjectRef(
                    module,
                    c"value".as_ptr(),
                    value,
                )
            };
            assert_eq!(result, 0);
            let stored = CROSSING_STORED.lock().unwrap()[0].2;
            assert_eq!(
                support::fake_runtime::ref_count(stored),
                1,
                "only the module's runtime edge remains"
            );
            assert_eq!((*value).ob_refcnt, if steal { 1 } else { 2 });
            let deallocs = CROSSING_DEALLOCS.load(Ordering::Relaxed);
            fixture.remove_values();
            if !steal {
                assert_eq!((*value).ob_refcnt, 1);
                assert_eq!(CROSSING_DEALLOCS.load(Ordering::Relaxed), deallocs);
                molt_cpython_abi::api::refcount::Py_DECREF(value);
            }
            assert_eq!(CROSSING_DEALLOCS.load(Ordering::Relaxed), deallocs + 1);
            assert!(CROSSING_FOREIGN.lock().unwrap().is_empty());
            molt_cpython_abi::api::refcount::Py_DECREF(module);
        }
    }
}

#[test]
fn module_failed_insertion_preserves_caller_reference_and_exact_error() {
    let _guard = init();
    let _fixture = CrossingFixture::new();
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let value = crossing_object(&mut typ);
    CROSSING_CLEANUP_ERROR.store(true, Ordering::Relaxed);
    unsafe {
        let module = molt_cpython_abi::api::modules::PyModule_New(c"custody_failure".as_ptr());
        for steal in [false, true] {
            let result = if steal {
                molt_cpython_abi::api::modules::PyModule_AddObject(
                    module,
                    c"reject_attr_with_error".as_ptr(),
                    value,
                )
            } else {
                molt_cpython_abi::api::modules::PyModule_AddObjectRef(
                    module,
                    c"reject_attr_with_error".as_ptr(),
                    value,
                )
            };
            assert_eq!(result, -1);
            let error = molt_cpython_abi::api::errors::take_current_error().unwrap();
            assert_eq!(error.exc_type, (&raw mut PyExc_ValueError).cast());
            assert_eq!(
                error.value.addr(),
                CROSSING_ERROR_VALUE.load(Ordering::Relaxed)
            );
            drop(error);
            assert_eq!((*value).ob_refcnt, 1);
            assert!(CROSSING_FOREIGN.lock().unwrap().is_empty());
        }
        assert_eq!(CROSSING_DEALLOCS.load(Ordering::Relaxed), 0);
        molt_cpython_abi::api::refcount::Py_DECREF(value);
        molt_cpython_abi::api::refcount::Py_DECREF(module);
    }
    assert_eq!(CROSSING_DEALLOCS.load(Ordering::Relaxed), 1);
}

#[test]
fn module_add_consumes_the_native_reference_even_when_insertion_fails() {
    let _guard = init();
    let _fixture = CrossingFixture::new();
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let value = crossing_object(&mut typ);
    let before = CROSSING_DEALLOCS.load(Ordering::Relaxed);
    unsafe {
        assert_eq!(
            molt_cpython_abi::api::modules::PyModule_Add(ptr::null_mut(), c"value".as_ptr(), value),
            -1
        );
        assert!(!molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
    }
    assert_eq!(CROSSING_DEALLOCS.load(Ordering::Relaxed), before + 1);
}

#[test]
fn module_crossing_allocation_failure_never_inserts_none_or_steals() {
    let _guard = init();
    let _fixture = CrossingFixture::new();
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let value = crossing_object(&mut typ);
    CROSSING_FAIL.store(true, Ordering::Relaxed);
    unsafe {
        let module = molt_cpython_abi::api::modules::PyModule_New(c"custody_oom".as_ptr());
        assert_eq!(
            molt_cpython_abi::api::modules::PyModule_AddObject(module, c"value".as_ptr(), value),
            -1
        );
        let error = molt_cpython_abi::api::errors::take_current_error().unwrap();
        assert_eq!(error.exc_type, (&raw mut PyExc_MemoryError).cast());
        drop(error);
        assert_eq!(CROSSING_SET_CALLS.load(Ordering::Relaxed), 0);
        assert_eq!((*value).ob_refcnt, 1);
        assert!(CROSSING_FOREIGN.lock().unwrap().is_empty());
        molt_cpython_abi::api::refcount::Py_DECREF(value);
        molt_cpython_abi::api::refcount::Py_DECREF(module);
    }
}

#[test]
fn module_receiver_crossings_retire_locals_for_lookup_and_state_registration() {
    let _guard = init();
    let _fixture = CrossingFixture::new();
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let module = crossing_object(&mut typ);
    CROSSING_CLEANUP_ERROR.store(true, Ordering::Relaxed);
    unsafe {
        assert!(molt_cpython_abi::api::modules::PyModule_GetDict(module).is_null());
        let error = molt_cpython_abi::api::errors::take_current_error().unwrap();
        assert_eq!(error.exc_type, (&raw mut PyExc_SystemError).cast());
        drop(error);
        assert_eq!((*module).ob_refcnt, 1);
        assert!(molt_cpython_abi::api::modules::PyModule_GetState(module).is_null());
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        assert_eq!((*module).ob_refcnt, 1);
        CROSSING_CLEANUP_ERROR.store(false, Ordering::Relaxed);
        let mut def: PyModuleDef = std::mem::zeroed();
        assert_eq!(
            molt_cpython_abi::api::modules::PyState_AddModule(module, &raw mut def),
            0
        );
        let stored = *FAKE_MODULE_STATE
            .lock()
            .unwrap()
            .by_def
            .get(&((&raw mut def) as usize))
            .unwrap();
        assert_eq!(
            support::fake_runtime::ref_count(stored),
            1,
            "registry owns its edge, local wrapper retired"
        );
        assert_eq!((*module).ob_refcnt, 2);
        assert_eq!(
            molt_cpython_abi::api::modules::PyState_RemoveModule(&raw mut def),
            0
        );
        assert_eq!((*module).ob_refcnt, 1);
        assert!(CROSSING_FOREIGN.lock().unwrap().is_empty());
        molt_cpython_abi::api::refcount::Py_DECREF(module);
    }
}

#[test]
fn test_module_addobject_null_module_returns_error() {
    let _guard = init();
    let val = unsafe { molt_cpython_abi::api::numbers::PyLong_FromLong(1) };
    let result = unsafe {
        molt_cpython_abi::api::modules::PyModule_AddObject(ptr::null_mut(), c"attr".as_ptr(), val)
    };
    assert_eq!(result, -1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(val) };
}

#[test]
fn test_module_addobject_null_name_returns_error() {
    let _guard = init();
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
    let _guard = init();
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
    let _guard = init();
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
    let _guard = init();
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
    let _guard = init();
    let result = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(ptr::null_mut()) };
    assert!(result.is_null());
}

#[test]
fn test_moduledef_init_returns_definition_pointer() {
    let _guard = init();
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

    // Take the raw `def` pointer ONCE and reuse it. `PyModuleDef_Init` returns
    // exactly this pointer (`(PyObject*)def`), so `out` aliases `def`. Forming a
    // fresh `&mut def as *mut PyModuleDef` for the comparison (as this test used
    // to) is a Unique retag over `def` that pops `out`'s tag off the borrow
    // stack, so the following `(*out).ob_refcnt` read is UB under Stacked
    // Borrows. Comparing against the pre-taken raw pointer keeps `out` live.
    let def_ptr: *mut PyModuleDef = &raw mut def;
    let out = unsafe { molt_cpython_abi::api::modules::PyModuleDef_Init(def_ptr) };

    assert_eq!(out.cast::<PyModuleDef>(), def_ptr);
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
    let next = unsafe { (*(state as *mut u8)).saturating_add(1) };
    unsafe { *(state as *mut u8) = next };
    MODULE_EXEC_CALLED.fetch_add(1, Ordering::Relaxed);
    MODULE_EXEC_STATE_BYTE.store(next.into(), Ordering::Relaxed);
    0
}

unsafe extern "C" fn fake_module_create_with_own_name(
    _spec: *mut PyObject,
    _def: *mut PyModuleDef,
) -> *mut PyObject {
    MODULE_CREATE_CALLED.fetch_add(1, Ordering::Relaxed);
    unsafe { molt_cpython_abi::api::modules::PyModule_New(c"custom.actual".as_ptr()) }
}

unsafe extern "C" fn fake_c_method(_self: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
    ptr::null_mut()
}

unsafe fn module_from_test_spec(def: *mut PyModuleDef) -> *mut PyObject {
    use molt_cpython_abi::api::{modules, refcount, strings};
    let spec = unsafe { modules::PyModule_New(c"spec".as_ptr()) };
    let name = unsafe { strings::PyUnicode_FromString((*def).m_name) };
    assert_eq!(
        unsafe { modules::PyModule_AddObjectRef(spec, c"name".as_ptr(), name) },
        0
    );
    let module = unsafe { modules::PyModule_FromDefAndSpec2(def, spec, 0) };
    unsafe {
        refcount::Py_DECREF(name);
        refcount::Py_DECREF(spec)
    };
    module
}

#[test]
fn test_module_from_def_and_spec_rejects_negative_state_before_create_slot() {
    let _guard = init();
    MODULE_CREATE_CALLED.store(0, Ordering::Relaxed);
    let capi_before = FAKE_MODULE_STATE.lock().unwrap().capi_by_module.len();
    let mut slots = [
        PyModuleDef_Slot {
            slot: 1, // CPython Py_mod_create
            value: fake_module_create_with_own_name as *mut c_void,
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
        m_name: c"invalid_multiphase_state".as_ptr(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: ptr::null_mut(),
        m_slots: slots.as_mut_ptr(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    let module = unsafe { module_from_test_spec(&mut def) };
    assert!(module.is_null());
    assert_eq!(MODULE_CREATE_CALLED.load(Ordering::Relaxed), 0);
    assert_eq!(
        FAKE_MODULE_STATE.lock().unwrap().capi_by_module.len(),
        capi_before,
        "invalid definition must not register partial module metadata"
    );
    let message = support::take_current_error_text().expect("invalid m_size must set an error");
    assert!(message.contains("m_size"), "{message}");
}

#[test]
fn test_module_create2_rejects_slots_before_module_construction() {
    let _guard = init();
    MODULE_CREATE_CALLED.store(0, Ordering::Relaxed);
    let capi_before = FAKE_MODULE_STATE.lock().unwrap().capi_by_module.len();
    let mut slots = [
        PyModuleDef_Slot {
            slot: 1, // CPython Py_mod_create
            value: fake_module_create_with_own_name as *mut c_void,
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
        m_name: c"invalid_singlephase_slots".as_ptr(),
        m_doc: ptr::null(),
        m_size: 0,
        m_methods: ptr::null_mut(),
        m_slots: slots.as_mut_ptr(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    let module = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(&mut def, 1013) };
    assert!(module.is_null());
    assert_eq!(MODULE_CREATE_CALLED.load(Ordering::Relaxed), 0);
    assert_eq!(
        FAKE_MODULE_STATE.lock().unwrap().capi_by_module.len(),
        capi_before,
        "invalid definition must not register partial module metadata"
    );
    let message = support::take_current_error_text().expect("m_slots must set an error");
    assert!(message.contains("m_slots"), "{message}");
}

#[test]
fn test_custom_module_create_preserves_own_name_but_methods_use_spec_name() {
    let _guard = init();
    MODULE_CREATE_CALLED.store(0, Ordering::Relaxed);
    let mut methods = [
        PyMethodDef {
            ml_name: c"probe".as_ptr(),
            ml_meth: Some(fake_c_method),
            ml_flags: METH_NOARGS,
            ml_doc: ptr::null(),
        },
        PyMethodDef {
            ml_name: ptr::null(),
            ml_meth: None,
            ml_flags: 0,
            ml_doc: ptr::null(),
        },
    ];
    let mut slots = [
        PyModuleDef_Slot {
            slot: 1, // CPython Py_mod_create
            value: fake_module_create_with_own_name as *mut c_void,
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
        m_name: c"qualified.requested".as_ptr(),
        m_doc: c"Multi-phase module documentation".as_ptr(),
        m_size: 0,
        m_methods: methods.as_mut_ptr(),
        m_slots: slots.as_mut_ptr(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    let module = unsafe { module_from_test_spec(&mut def) };
    assert!(!module.is_null());
    assert_eq!(MODULE_CREATE_CALLED.load(Ordering::Relaxed), 1);
    unsafe {
        let actual_name = molt_cpython_abi::api::modules::PyModule_GetName(module);
        assert!(!actual_name.is_null());
        assert_eq!(std::ffi::CStr::from_ptr(actual_name), c"custom.actual");
        let dict = molt_cpython_abi::api::modules::PyModule_GetDict(module);
        let doc = molt_cpython_abi::api::mapping::PyDict_GetItemString(dict, c"__doc__".as_ptr());
        assert!(!doc.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(molt_cpython_abi::api::strings::PyUnicode_AsUTF8(doc)),
            c"Multi-phase module documentation"
        );
        let method = molt_cpython_abi::api::mapping::PyDict_GetItemString(dict, c"probe".as_ptr());
        assert!(!method.is_null());
        assert_eq!((*method).ob_type, &raw mut PyCFunction_Type);
        let physical = method.cast::<PyCFunctionObject>();
        assert_eq!((*physical).m_self, module);
        let method_module = molt_cpython_abi::api::strings::PyUnicode_AsUTF8((*physical).m_module);
        assert!(!method_module.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(method_module),
            c"qualified.requested"
        );
        molt_cpython_abi::api::refcount::Py_DECREF(module);
    }
}

#[test]
fn test_module_from_def_and_spec_defers_py_mod_exec_slot() {
    let _guard = init();
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
        m_size: 0,
        m_methods: ptr::null_mut(),
        m_slots: slots.as_mut_ptr(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };

    let module = unsafe { module_from_test_spec(&mut def) };

    assert!(!module.is_null());
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 0);
    assert!(unsafe { molt_cpython_abi::api::modules::PyState_FindModule(&mut def) }.is_null());
    // CPython rejects the single-phase registry for any slotted definition.
    unsafe {
        assert_eq!(
            molt_cpython_abi::api::modules::PyState_AddModule(module, &mut def),
            -1
        );
        assert_eq!(
            molt_cpython_abi::api::errors::PyErr_Occurred(),
            (&raw mut PyExc_SystemError).cast::<PyObject>()
        );
        molt_cpython_abi::api::errors::PyErr_Clear();
        assert!(molt_cpython_abi::api::modules::PyState_FindModule(&mut def).is_null());
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
        assert_eq!(
            molt_cpython_abi::api::modules::PyState_RemoveModule(&mut def),
            -1
        );
        assert_eq!(
            molt_cpython_abi::api::errors::PyErr_Occurred(),
            (&raw mut PyExc_SystemError).cast::<PyObject>()
        );
        molt_cpython_abi::api::errors::PyErr_Clear();
    }
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyModule_ExecDef(module, &mut def) },
        0
    );
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module) };
}

#[test]
fn test_module_from_def_and_spec_accepts_python312_metadata_slots() {
    let _guard = init();
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
            value: std::ptr::dangling_mut::<c_void>(),
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
        m_size: 0,
        m_methods: ptr::null_mut(),
        m_slots: slots.as_mut_ptr(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };

    let module = unsafe { module_from_test_spec(&mut def) };

    assert!(!module.is_null());
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 0);
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyModule_ExecDef(module, &mut def) },
        0
    );
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 1);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module) };
}

#[test]
fn test_module_from_def_and_spec_defers_state_and_direct_exec_reenters_slots() {
    let _guard = init();
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

    let module = unsafe { module_from_test_spec(&mut def) };

    assert!(!module.is_null());
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 0);
    assert_eq!(MODULE_EXEC_STATE_BYTE.load(Ordering::Relaxed), 0);
    assert!(unsafe { molt_cpython_abi::api::modules::PyModule_GetState(module) }.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyModule_ExecDef(module, &mut def) },
        0
    );
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 1);
    assert_eq!(MODULE_EXEC_STATE_BYTE.load(Ordering::Relaxed), 1);
    let state = unsafe { molt_cpython_abi::api::modules::PyModule_GetState(module) };
    assert!(!state.is_null());
    assert_eq!(unsafe { *(state as *mut u8) }, 1);
    // CPython's direct PyModule_ExecDef runs slots again. Only the import
    // loader's _imp.exec_dynamic gate skips an already-entered module.
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyModule_ExecDef(module, &mut def) },
        0
    );
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 2);
    assert_eq!(MODULE_EXEC_STATE_BYTE.load(Ordering::Relaxed), 2);
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyModule_GetState(module) },
        state,
        "direct re-execution must keep the same allocated module state"
    );
    assert_eq!(unsafe { *(state as *mut u8) }, 2);
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module) };
}

#[test]
fn test_module_from_def_and_spec_exec_failure_sets_error_message() {
    let _guard = init();
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
        m_size: 0,
        m_methods: ptr::null_mut(),
        m_slots: slots.as_mut_ptr(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };

    let module = unsafe { module_from_test_spec(&mut def) };

    assert!(!module.is_null());
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 0);
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyModule_ExecDef(module, &mut def) },
        -1
    );
    assert_eq!(MODULE_EXEC_CALLED.load(Ordering::Relaxed), 1);
    let message = support::take_current_error_text()
        .expect("exec failure must enter CPython ABI error state");
    assert!(message.contains("Py_mod_exec slot returned non-zero"));
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(module) };
}

// ---------------------------------------------------------------------------
// PyModule_Create2
// ---------------------------------------------------------------------------

#[test]
fn test_module_create2_null_returns_null() {
    let _guard = init();
    let result = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(ptr::null_mut(), 0) };
    assert!(result.is_null());
}

#[test]
fn test_module_create2_with_valid_def() {
    let _guard = init();
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
        m_doc: c"Single-phase module documentation".as_ptr(),
        m_size: -1,
        m_methods: ptr::null_mut(),
        m_slots: ptr::null_mut(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    let m = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(&mut def, 1013) };
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyModule_GetDef(m) },
        &raw mut def
    );
    assert!(!m.is_null());
    unsafe {
        let dict = molt_cpython_abi::api::modules::PyModule_GetDict(m);
        let doc = molt_cpython_abi::api::mapping::PyDict_GetItemString(dict, c"__doc__".as_ptr());
        assert!(!doc.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(molt_cpython_abi::api::strings::PyUnicode_AsUTF8(doc)),
            c"Single-phase module documentation"
        );
    }
    assert!(unsafe { molt_cpython_abi::api::modules::PyState_FindModule(&mut def) }.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

#[test]
fn test_module_create2_state_is_independent_of_explicit_pystate_registry_roundtrip() {
    let _guard = init();
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

    let created_found = unsafe { molt_cpython_abi::api::modules::PyState_FindModule(def) };
    assert!(
        created_found.is_null(),
        "PyModule_Create2 is not a successful import commit"
    );

    // The explicit PyState API remains a separate registry authority.
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyState_AddModule(m, def) },
        0
    );
    let found = unsafe { molt_cpython_abi::api::modules::PyState_FindModule(def) };
    assert!(!found.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::modules::PyState_RemoveModule(def) },
        0
    );
    assert!(unsafe { molt_cpython_abi::api::modules::PyState_FindModule(def) }.is_null());

    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

#[test]
fn test_module_create2_null_name_uses_unnamed() {
    let _guard = init();
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
    assert!(unsafe { molt_cpython_abi::api::modules::PyState_FindModule(&mut def) }.is_null());
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(m) };
}

#[test]
fn test_module_create2_callable_publication_failures_preserve_the_original_error() {
    let _guard = init();
    for (name, flags, expected_type, expected_message) in [
        (
            c"reject",
            METH_VARARGS,
            (&raw mut PyExc_SystemError).cast::<PyObject>(),
            "C function runtime registration failed",
        ),
        (
            c"reject_attr",
            METH_VARARGS,
            (&raw mut PyExc_SystemError).cast::<PyObject>(),
            "module attribute assignment returned non-zero",
        ),
        (
            c"reject_with_error",
            METH_VARARGS,
            (&raw mut PyExc_MemoryError).cast::<PyObject>(),
            "callable allocation detail",
        ),
        (
            c"reject_attr_with_error",
            METH_VARARGS,
            (&raw mut PyExc_ValueError).cast::<PyObject>(),
            "method publication detail",
        ),
        (
            c"class_bound",
            METH_VARARGS | METH_CLASS,
            (&raw mut PyExc_ValueError).cast::<PyObject>(),
            "module functions cannot set METH_CLASS or METH_STATIC",
        ),
    ] {
        let mut methods = [
            PyMethodDef {
                ml_name: name.as_ptr(),
                ml_meth: Some(fake_c_method),
                ml_flags: flags,
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
            m_name: c"publication_failure".as_ptr(),
            m_doc: ptr::null(),
            m_size: -1,
            m_methods: methods.as_mut_ptr(),
            m_slots: ptr::null_mut(),
            m_traverse: ptr::null_mut(),
            m_clear: ptr::null_mut(),
            m_free: ptr::null_mut(),
        };
        let module = unsafe { molt_cpython_abi::api::modules::PyModule_Create2(&mut def, 1013) };
        assert!(module.is_null(), "{name:?}");
        assert_ne!(
            unsafe { molt_cpython_abi::api::errors::PyErr_ExceptionMatches(expected_type) },
            0,
            "{name:?}",
        );
        let message = support::take_current_error_text()
            .expect("publication failure must leave an exception");
        assert!(message.contains(expected_message), "{name:?}: {message}");
    }
}

#[test]
fn test_module_create2_methods_are_canonical_cfunction_views_that_outlive_constructor_refs() {
    let _guard = init();
    let _fixture = CrossingFixture::new();
    let mut methods = [
        PyMethodDef {
            ml_name: c"alpha".as_ptr(),
            ml_meth: Some(fake_c_method),
            ml_flags: METH_VARARGS,
            ml_doc: c"alpha doc".as_ptr(),
        },
        PyMethodDef {
            ml_name: c"beta".as_ptr(),
            ml_meth: Some(fake_c_method),
            ml_flags: METH_NOARGS,
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
        m_name: c"canonical_methods".as_ptr(),
        m_doc: ptr::null(),
        m_size: -1,
        m_methods: methods.as_mut_ptr(),
        m_slots: ptr::null_mut(),
        m_traverse: ptr::null_mut(),
        m_clear: ptr::null_mut(),
        m_free: ptr::null_mut(),
    };
    unsafe {
        let module = molt_cpython_abi::api::modules::PyModule_Create2(&mut def, 1013);
        assert!(!module.is_null());
        // CPython oracle: the caller owns one reference and each module
        // function's PyCFunctionObject.m_self owns another.
        assert_eq!((*module).ob_refcnt, 3);
        let dict = molt_cpython_abi::api::modules::PyModule_GetDict(module);
        assert!(!dict.is_null());
        let lookup = |name: &std::ffi::CStr| {
            molt_cpython_abi::api::mapping::PyDict_GetItemString(dict, name.as_ptr())
        };
        let module_name = lookup(c"__name__");
        assert!(!module_name.is_null());
        let alpha = lookup(c"alpha");
        let beta = lookup(c"beta");
        assert_ne!(alpha, beta);
        for (function, definition) in [(alpha, &raw mut methods[0]), (beta, &raw mut methods[1])] {
            assert!(!function.is_null());
            assert_eq!(
                (*function).ob_type,
                &raw mut PyCFunction_Type,
                "a module-dict round trip must return the physical PyCFunctionObject"
            );
            let physical = function.cast::<PyCFunctionObject>();
            assert_eq!((*physical).m_ml, definition);
            assert_eq!((*physical).m_self, module);
            assert_eq!((*physical).m_module, module_name);
            assert_eq!(
                molt_cpython_abi::api::object::PyCFunction_GetSelf(function),
                module
            );
            assert_eq!(
                molt_cpython_abi::api::object::PyCFunction_GetFlags(function),
                (*definition).ml_flags
            );
            let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .molt_handle_for_pyobj(function)
                .expect("module function keeps its runtime identity")
                .bits();
            let mut gc_edges = Vec::new();
            molt_cpython_abi::bridge::GLOBAL_BRIDGE
                .visit_physical_owned_edges_for_gc(bits, &mut |edge| gc_edges.push(edge));
            assert_eq!(gc_edges.len(), 1, "m_module is the only independent C edge");
            assert_eq!(
                gc_edges[0].kind,
                molt_cpython_abi::NativeGcEdgeKind::ManagedHandle as u8
            );
            assert_eq!(
                gc_edges[0].value,
                molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(module_name)
                    .unwrap()
                    .bits(),
            );
        }
        assert_eq!(
            std::ffi::CStr::from_ptr((*(*alpha.cast::<PyCFunctionObject>()).m_ml).ml_doc),
            c"alpha doc"
        );
        // Releasing the caller reference leaves the module owned by the
        // retained native m_self edges, and every method keeps its identity.
        molt_cpython_abi::api::refcount::Py_DECREF(module);
        assert_eq!((*module).ob_refcnt, 2);
        assert_eq!(lookup(c"alpha"), alpha);
        assert_eq!(lookup(c"beta"), beta);
        let retained = (*alpha.cast::<PyCFunctionObject>()).m_self;
        assert_eq!(retained, module);
        assert_eq!(
            molt_cpython_abi::api::modules::PyModule_GetDict(retained),
            dict
        );
        assert!(
            molt_cpython_abi::api::modules::PyState_FindModule(&mut def).is_null(),
            "constructor-owned C functions do not imply PyState registry custody"
        );
    }
}

struct ModulePublicationObservation {
    dict: u64,
    key: u64,
    expected: Option<u64>,
    displaced: u64,
    committed: bool,
    owner_alive: bool,
    deallocs: u64,
    calls: usize,
    fail: bool,
    error_value: usize,
}

unsafe extern "C" fn observe_module_publication(context: *mut c_void) -> i32 {
    use molt_cpython_abi::hooks::{DecodedHandleResult, DictHashSource};
    let observation = unsafe { &mut *context.cast::<ModulePublicationObservation>() };
    let hooks = molt_cpython_abi::hooks::hooks_or_stubs();
    observation.calls += 1;
    observation.committed = match unsafe {
        (hooks.dict_get)(
            observation.dict,
            observation.key,
            DictHashSource::Compute,
            0,
        )
    }
    .decode()
    {
        DecodedHandleResult::Ok(value) => observation.expected == Some(value),
        DecodedHandleResult::Missing => observation.expected.is_none(),
        DecodedHandleResult::Error => false,
    };
    observation.owner_alive = unsafe { (hooks.ref_count)(observation.displaced) == 1 };
    observation.deallocs = CROSSING_DEALLOCS.load(Ordering::Relaxed);
    if observation.fail {
        unsafe {
            molt_cpython_abi::api::errors::PyErr_SetString(
                (&raw mut PyExc_ValueError).cast(),
                c"module publication rejected".as_ptr(),
            );
        }
        let error = molt_cpython_abi::api::errors::take_current_error().unwrap();
        observation.error_value = error.value.addr();
        molt_cpython_abi::api::errors::restore_current_error_exact(error);
        -1
    } else {
        0
    }
}

#[test]
fn module_dictionary_publication_precedes_foreign_retirement_and_preserves_error() {
    let _guard = init();
    let _fixture = CrossingFixture::new();
    let hooks = molt_cpython_abi::hooks::hooks_or_stubs();
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    unsafe {
        use molt_cpython_abi::hooks::{DecodedHandleResult, DictHashSource};
        let module = molt_cpython_abi::api::modules::PyModule_New(c"publication".as_ptr());
        let old_object = crossing_object(&mut typ);
        assert_eq!(
            molt_cpython_abi::api::modules::PyModule_AddObject(
                module,
                c"value".as_ptr(),
                old_object
            ),
            0
        );
        let (dict, old) = {
            let stored = CROSSING_STORED.lock().unwrap();
            (stored[0].0, stored[0].2)
        };
        let (mut original_key, mut original_value) = (0, 0);
        assert_eq!(
            (hooks.dict_entry)(dict, 1, &mut original_key, &mut original_value),
            1
        );
        assert_eq!(original_value, old);
        // A separately allocated equal name exercises the namespace's content
        // lookup while preserving the original dictionary key owner.
        let key = support::fake_runtime::alloc_str(b"value".as_ptr(), 5);
        assert_ne!(key, original_key);
        let replacement = support::fake_runtime::fresh_handle();
        let mut observation = ModulePublicationObservation {
            dict,
            key,
            expected: Some(replacement),
            displaced: old,
            committed: false,
            owner_alive: false,
            deallocs: 0,
            calls: 0,
            fail: true,
            error_value: 0,
        };
        CROSSING_CLEANUP_ERROR.store(true, Ordering::Relaxed);
        assert_eq!(
            (hooks.dict_mutate)(
                dict,
                key,
                replacement,
                0,
                Some(observe_module_publication),
                (&raw mut observation).cast()
            ),
            -1
        );
        assert!(observation.committed && observation.owner_alive);
        assert_eq!(observation.calls, 1);
        assert_eq!(observation.deallocs, 0);
        assert_eq!(CROSSING_DEALLOCS.load(Ordering::Relaxed), 1);
        assert_eq!(support::fake_runtime::ref_count(old), 0);
        let error = molt_cpython_abi::api::errors::take_current_error().unwrap();
        assert_eq!(error.exc_type, (&raw mut PyExc_ValueError).cast());
        assert_eq!(
            error.value.addr(),
            observation.error_value,
            "retirement's KeyError must not replace the publication error"
        );
        drop(error);
        CROSSING_CLEANUP_ERROR.store(false, Ordering::Relaxed);
        assert!(
            matches!((hooks.dict_get)(dict, original_key, DictHashSource::Compute, 0).decode(), DecodedHandleResult::Ok(value) if value == replacement)
        );
        let mut stored_key = 0;
        let mut stored_value = 0;
        assert_eq!(
            (hooks.dict_entry)(dict, 1, &mut stored_key, &mut stored_value),
            1
        );
        assert_eq!(stored_key, original_key);
        assert_eq!(stored_value, replacement);
        assert_eq!((hooks.dict_len)(dict), 2);
        support::fake_runtime::dec_ref(replacement);
        observation.expected = None;
        observation.displaced = replacement;
        observation.fail = false;
        assert_eq!(
            (hooks.dict_mutate)(
                dict,
                key,
                0,
                1,
                Some(observe_module_publication),
                (&raw mut observation).cast()
            ),
            0
        );
        assert!(observation.committed && observation.owner_alive);
        assert_eq!(observation.calls, 2);
        assert_eq!(support::fake_runtime::ref_count(replacement), 0);
        assert_eq!((hooks.dict_len)(dict), 1);
        assert_eq!(
            (hooks.dict_mutate)(
                dict,
                key,
                0,
                1,
                Some(observe_module_publication),
                (&raw mut observation).cast()
            ),
            1
        );
        assert_eq!(observation.calls, 2);
        support::fake_runtime::dec_ref(key);
        molt_cpython_abi::api::refcount::Py_DECREF(module);
        assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
    }
}
