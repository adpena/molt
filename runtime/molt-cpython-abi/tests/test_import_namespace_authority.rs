//! Teeth for the single import-namespace authority.
//!
//! The fake hooks model the borrowed/owned boundary and the runtime's atomic
//! AddModule transaction. The test catches detached namespace fallbacks,
//! non-module cache passthrough, partial publication, import re-entry from
//! PyEval_GetBuiltins, and loss of exact hook exceptions.

use molt_cpython_abi::abi_types::{MoltTypeTag, PyObject};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
use molt_cpython_abi::hooks::{BorrowedHandleResult, RuntimeHooks};
use molt_lang_obj_model::MoltObject;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

static STRINGS: LazyLock<Mutex<HashMap<u64, Vec<u8>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static INTERNED: LazyLock<Mutex<HashMap<Vec<u8>, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static DICTS: LazyLock<Mutex<HashMap<u64, HashMap<u64, u64>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static MODULES: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static MODULE_DICTS: LazyLock<Mutex<HashMap<u64, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static REFCOUNTS: LazyLock<Mutex<HashMap<u64, usize>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static SYS_MODULES: Mutex<u64> = Mutex::new(0);
static DEFAULT_BUILTINS: Mutex<u64> = Mutex::new(0);
static ACTIVE_FRAME_BUILTINS: Mutex<u64> = Mutex::new(0);
static FAIL_SYS_MODULES: AtomicBool = AtomicBool::new(false);
static FAIL_ADD_PUBLICATION: AtomicBool = AtomicBool::new(false);
static FAIL_BUILTINS_LOOKUP: AtomicBool = AtomicBool::new(false);
static ADD_CALLS: AtomicUsize = AtomicUsize::new(0);
static IMPORT_CALLS: AtomicUsize = AtomicUsize::new(0);
static MODULE_ALLOCS: AtomicUsize = AtomicUsize::new(0);
static MODULE_DECREFS: AtomicUsize = AtomicUsize::new(0);

fn fresh_handle() -> u64 {
    let ptr: *mut u64 = Box::leak(Box::new(0u64));
    let bits = MoltObject::from_ptr(ptr.cast::<u8>()).bits();
    REFCOUNTS.lock().unwrap().insert(bits, 1);
    bits
}

unsafe extern "C" fn inc_ref(bits: u64) {
    let mut refs = REFCOUNTS.lock().unwrap();
    let count = refs.get_mut(&bits).expect("retained handle must be live");
    *count = count.checked_add(1).expect("test refcount overflow");
}

unsafe extern "C" fn dec_ref(bits: u64) {
    let is_module = MODULES.lock().unwrap().contains(&bits);
    if is_module {
        MODULE_DECREFS.fetch_add(1, Ordering::SeqCst);
    }
    let reached_zero = {
        let mut refs = REFCOUNTS.lock().unwrap();
        let count = refs.get_mut(&bits).expect("released handle must be live");
        assert!(*count > 0, "test refcount underflow for {bits:#x}");
        *count -= 1;
        *count == 0
    };
    if reached_zero && is_module {
        MODULES.lock().unwrap().remove(&bits);
        if let Some(dict) = MODULE_DICTS.lock().unwrap().remove(&bits) {
            unsafe { dec_ref(dict) };
        }
    }
}

unsafe extern "C" fn ref_count(bits: u64) -> usize {
    REFCOUNTS.lock().unwrap().get(&bits).copied().unwrap_or(0)
}

unsafe extern "C" fn alloc_str(data: *const u8, len: usize) -> u64 {
    let bytes = if data.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    };
    if let Some(bits) = INTERNED.lock().unwrap().get(bytes).copied()
        && unsafe { ref_count(bits) } != 0
    {
        unsafe { inc_ref(bits) };
        return bits;
    }
    let bits = fresh_handle();
    let owned = bytes.to_vec();
    STRINGS.lock().unwrap().insert(bits, owned.clone());
    INTERNED.lock().unwrap().insert(owned, bits);
    bits
}

unsafe extern "C" fn str_data(bits: u64, out_len: *mut usize) -> *const u8 {
    let strings = STRINGS.lock().unwrap();
    let Some(bytes) = strings.get(&bits) else {
        return std::ptr::null();
    };
    unsafe { *out_len = bytes.len() };
    bytes.as_ptr()
}

unsafe extern "C" fn classify_heap(bits: u64) -> u8 {
    if STRINGS.lock().unwrap().contains_key(&bits) {
        MoltTypeTag::Str as u8
    } else if DICTS.lock().unwrap().contains_key(&bits) {
        MoltTypeTag::Dict as u8
    } else if MODULES.lock().unwrap().contains(&bits) {
        MoltTypeTag::Module as u8
    } else {
        MoltTypeTag::Other as u8
    }
}

fn alloc_dict_handle() -> u64 {
    let bits = fresh_handle();
    DICTS.lock().unwrap().insert(bits, HashMap::new());
    bits
}

fn store_dict_entry(dict: u64, key: u64, value: u64) -> bool {
    let previous = {
        let mut dicts = DICTS.lock().unwrap();
        let Some(entries) = dicts.get_mut(&dict) else {
            return false;
        };
        match entries.entry(key) {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                unsafe { inc_ref(value) };
                Some(entry.insert(value))
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                unsafe {
                    inc_ref(key);
                    inc_ref(value);
                }
                entry.insert(value);
                None
            }
        }
    };
    if let Some(previous) = previous {
        unsafe { dec_ref(previous) };
    }
    true
}

fn lookup_name(dict: u64, name: &[u8]) -> Option<u64> {
    let key = INTERNED.lock().unwrap().get(name).copied()?;
    DICTS.lock().unwrap().get(&dict)?.get(&key).copied()
}

fn set_exact_hook_error(exc_type: *mut PyObject) {
    unsafe {
        // Runtime hooks report through the canonical error channel; the mock
        // must not re-enter PyErr_SetString, whose normalization intentionally
        // calls the runtime exception hooks that this focused harness omits.
        molt_cpython_abi::api::refcount::Py_INCREF(exc_type);
        molt_cpython_abi::api::errors::restore_current_error_exact(
            molt_cpython_abi::api::errors::OwnedCError {
                exc_type,
                value: std::ptr::null_mut(),
                traceback: std::ptr::null_mut(),
            },
        );
    }
}

unsafe extern "C" fn sys_get_object_borrowed(data: *const u8, len: usize) -> BorrowedHandleResult {
    let name = unsafe { std::slice::from_raw_parts(data, len) };
    if name != b"modules" {
        return BorrowedHandleResult::missing();
    }
    if FAIL_SYS_MODULES.load(Ordering::SeqCst) {
        set_exact_hook_error(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast::<PyObject>(),
        );
        return BorrowedHandleResult::error();
    }
    let modules = *SYS_MODULES.lock().unwrap();
    if modules == 0 {
        set_exact_hook_error(
            (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>(),
        );
        BorrowedHandleResult::error()
    } else {
        BorrowedHandleResult::ok(modules)
    }
}

unsafe extern "C" fn alloc_module(data: *const u8, len: usize) -> u64 {
    let _name = unsafe { std::slice::from_raw_parts(data, len) };
    MODULE_ALLOCS.fetch_add(1, Ordering::SeqCst);
    let bits = fresh_handle();
    MODULES.lock().unwrap().insert(bits);
    MODULE_DICTS
        .lock()
        .unwrap()
        .insert(bits, alloc_dict_handle());
    bits
}

unsafe extern "C" fn module_get_dict_borrowed(module: u64) -> BorrowedHandleResult {
    if let Some(dict) = MODULE_DICTS.lock().unwrap().get(&module).copied() {
        return BorrowedHandleResult::ok(dict);
    }
    set_exact_hook_error(
        (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>(),
    );
    BorrowedHandleResult::error()
}

unsafe extern "C" fn import_add_module_borrowed(
    data: *const u8,
    len: usize,
) -> BorrowedHandleResult {
    ADD_CALLS.fetch_add(1, Ordering::SeqCst);
    let name = unsafe { std::slice::from_raw_parts(data, len) };
    let modules = *SYS_MODULES.lock().unwrap();
    if modules == 0 {
        set_exact_hook_error(
            (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>(),
        );
        return BorrowedHandleResult::error();
    }
    let key = unsafe { alloc_str(name.as_ptr(), name.len()) };
    let existing = DICTS
        .lock()
        .unwrap()
        .get(&modules)
        .and_then(|entries| entries.get(&key).copied());
    if let Some(existing) = existing
        && MODULES.lock().unwrap().contains(&existing)
    {
        unsafe { dec_ref(key) };
        return BorrowedHandleResult::ok(existing);
    }
    let module = unsafe { alloc_module(name.as_ptr(), name.len()) };
    if FAIL_ADD_PUBLICATION.load(Ordering::SeqCst) {
        set_exact_hook_error(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast::<PyObject>(),
        );
        unsafe {
            dec_ref(module);
            dec_ref(key);
        }
        return BorrowedHandleResult::error();
    }
    assert!(store_dict_entry(modules, key, module));
    unsafe {
        dec_ref(module);
        dec_ref(key);
    }
    BorrowedHandleResult::ok(module)
}

unsafe extern "C" fn eval_get_builtins_borrowed() -> BorrowedHandleResult {
    if FAIL_BUILTINS_LOOKUP.load(Ordering::SeqCst) {
        set_exact_hook_error(
            (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast::<PyObject>(),
        );
        return BorrowedHandleResult::error();
    }
    let active = *ACTIVE_FRAME_BUILTINS.lock().unwrap();
    let bits = if active == 0 {
        *DEFAULT_BUILTINS.lock().unwrap()
    } else {
        active
    };
    if bits == 0 {
        set_exact_hook_error(
            (&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>(),
        );
        BorrowedHandleResult::error()
    } else {
        BorrowedHandleResult::ok(bits)
    }
}

unsafe extern "C" fn import_module(_data: *const u8, _len: usize) -> u64 {
    IMPORT_CALLS.fetch_add(1, Ordering::SeqCst);
    0
}

fn bits_of(object: *mut PyObject) -> u64 {
    GLOBAL_BRIDGE
        .pyobj_to_handle(object)
        .map(|identity| identity.as_handle())
        .expect("object must be backed by the installed runtime")
}

fn assert_matches(exception: *mut PyObject) {
    assert_eq!(
        unsafe { molt_cpython_abi::api::errors::PyErr_ExceptionMatches(exception) },
        1,
        "the exact hook exception must survive the ABI boundary"
    );
}

#[test]
fn imports_share_one_namespace_and_publication_is_transactional() {
    let imports_source = include_str!("../src/api/imports.rs");
    let eval_source = include_str!("../src/api/eval.rs");
    assert!(!imports_source.contains("static MODULE_DICT"));
    assert!(!eval_source.contains("static BUILTINS_DICT"));
    assert!(!eval_source.contains("PyImport_ImportModule"));
    assert!(eval_source.contains("eval_get_builtins_borrowed"));
    assert!(imports_source.contains("import_add_module_borrowed"));
    assert!(imports_source.contains("std::str::from_utf8(name_bytes)"));

    let sys_modules = alloc_dict_handle();
    *SYS_MODULES.lock().unwrap() = sys_modules;
    let default_builtins = alloc_dict_handle();
    let frame_builtins = alloc_dict_handle();
    *DEFAULT_BUILTINS.lock().unwrap() = default_builtins;

    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.alloc_str = alloc_str;
    hooks.str_data = str_data;
    hooks.classify_heap = classify_heap;
    hooks.sys_get_object_borrowed = sys_get_object_borrowed;
    hooks.eval_get_builtins_borrowed = eval_get_builtins_borrowed;
    hooks.inc_ref = inc_ref;
    hooks.dec_ref = dec_ref;
    hooks.ref_count = ref_count;
    hooks.alloc_module = alloc_module;
    hooks.module_get_dict_borrowed = module_get_dict_borrowed;
    hooks.import_add_module_borrowed = import_add_module_borrowed;
    hooks.import_module = import_module;
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    assert!(unsafe { molt_cpython_abi::try_set_runtime_hooks(hooks) });

    let modules = unsafe { molt_cpython_abi::api::imports::PyImport_GetModuleDict() };
    assert_eq!(bits_of(modules), sys_modules);
    let modules_refs = unsafe { ref_count(sys_modules) };
    assert_eq!(
        modules,
        unsafe { molt_cpython_abi::api::imports::PyImport_GetModuleDict() },
        "GetModuleDict must return one stable borrowed sys.modules view"
    );
    assert_eq!(
        unsafe { ref_count(sys_modules) },
        modules_refs,
        "re-borrowing an existing ABI view must not mint an owned edge"
    );

    FAIL_SYS_MODULES.store(true, Ordering::SeqCst);
    let failed_modules = unsafe { molt_cpython_abi::api::imports::PyImport_GetModuleDict() };
    assert!(failed_modules.is_null());
    assert_matches((&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast::<PyObject>());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    FAIL_SYS_MODULES.store(false, Ordering::SeqCst);

    *SYS_MODULES.lock().unwrap() = 0;
    let missing_modules = unsafe { molt_cpython_abi::api::imports::PyImport_GetModuleDict() };
    assert!(missing_modules.is_null());
    assert_matches((&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    *SYS_MODULES.lock().unwrap() = sys_modules;

    FAIL_ADD_PUBLICATION.store(true, Ordering::SeqCst);
    let allocations_before_failure = MODULE_ALLOCS.load(Ordering::SeqCst);
    let decrefs_before_failure = MODULE_DECREFS.load(Ordering::SeqCst);
    let failed = unsafe { molt_cpython_abi::api::imports::PyImport_AddModule(c"fails".as_ptr()) };
    assert!(failed.is_null());
    assert_matches((&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast::<PyObject>());
    assert_eq!(
        MODULE_ALLOCS.load(Ordering::SeqCst),
        allocations_before_failure + 1
    );
    assert_eq!(
        MODULE_DECREFS.load(Ordering::SeqCst),
        decrefs_before_failure + 1,
        "the unpublishable fresh module must release its owned edge"
    );
    assert_eq!(lookup_name(sys_modules, b"fails"), None);
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    FAIL_ADD_PUBLICATION.store(false, Ordering::SeqCst);

    let allocations_before_success = MODULE_ALLOCS.load(Ordering::SeqCst);
    let first =
        unsafe { molt_cpython_abi::api::imports::PyImport_AddModule(c"published".as_ptr()) };
    assert!(!first.is_null());
    let first_bits = bits_of(first);
    assert_eq!(
        MODULE_ALLOCS.load(Ordering::SeqCst),
        allocations_before_success + 1
    );
    assert_eq!(lookup_name(sys_modules, b"published"), Some(first_bits));
    let first_refs = unsafe { ref_count(first_bits) };
    let again =
        unsafe { molt_cpython_abi::api::imports::PyImport_AddModule(c"published".as_ptr()) };
    assert_eq!(first, again);
    assert_eq!(
        unsafe { ref_count(first_bits) },
        first_refs,
        "AddModule returns borrowed on both the cold and cached paths"
    );
    assert_eq!(
        MODULE_ALLOCS.load(Ordering::SeqCst),
        allocations_before_success + 1,
        "an existing module must be returned without a second allocation"
    );

    let replacement_key = unsafe { alloc_str(b"replacement".as_ptr(), b"replacement".len()) };
    let non_module = alloc_dict_handle();
    assert!(store_dict_entry(sys_modules, replacement_key, non_module));
    unsafe {
        dec_ref(replacement_key);
        dec_ref(non_module);
    }
    let replacement =
        unsafe { molt_cpython_abi::api::imports::PyImport_AddModule(c"replacement".as_ptr()) };
    assert!(!replacement.is_null());
    let replacement_bits = bits_of(replacement);
    assert_ne!(replacement_bits, non_module);
    assert_eq!(
        lookup_name(sys_modules, b"replacement"),
        Some(replacement_bits)
    );
    assert_eq!(
        unsafe { classify_heap(replacement_bits) },
        MoltTypeTag::Module as u8,
        "AddModule must replace a cached non-module with a fresh module"
    );

    let builtins = unsafe { molt_cpython_abi::api::eval::PyEval_GetBuiltins() };
    assert_eq!(bits_of(builtins), default_builtins);
    let default_builtins_refs = unsafe { ref_count(default_builtins) };
    *ACTIVE_FRAME_BUILTINS.lock().unwrap() = frame_builtins;
    let active_builtins = unsafe { molt_cpython_abi::api::eval::PyEval_GetBuiltins() };
    assert_eq!(bits_of(active_builtins), frame_builtins);
    *ACTIVE_FRAME_BUILTINS.lock().unwrap() = 0;
    assert_eq!(
        bits_of(unsafe { molt_cpython_abi::api::eval::PyEval_GetBuiltins() }),
        default_builtins
    );
    assert_eq!(
        unsafe { ref_count(default_builtins) },
        default_builtins_refs,
        "PyEval_GetBuiltins returns a stable borrowed view"
    );
    assert_eq!(
        IMPORT_CALLS.load(Ordering::SeqCst),
        0,
        "PyEval_GetBuiltins must not re-enter the import system"
    );

    FAIL_BUILTINS_LOOKUP.store(true, Ordering::SeqCst);
    let failed_builtins = unsafe { molt_cpython_abi::api::eval::PyEval_GetBuiltins() };
    assert!(failed_builtins.is_null());
    assert_matches((&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast::<PyObject>());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    FAIL_BUILTINS_LOOKUP.store(false, Ordering::SeqCst);

    *DEFAULT_BUILTINS.lock().unwrap() = 0;
    let missing_builtins = unsafe { molt_cpython_abi::api::eval::PyEval_GetBuiltins() };
    assert!(missing_builtins.is_null());
    assert_matches((&raw mut molt_cpython_abi::abi_types::PyExc_SystemError).cast::<PyObject>());
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    *DEFAULT_BUILTINS.lock().unwrap() = default_builtins;
}
