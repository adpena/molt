//! PyType_Ready static-type readiness contract, exercised against the exact
//! shape numpy's `setup_scalartypes` relies on: a multi-level `tp_base` chain
//! of statically declared types, each with `tp_methods`/`tp_getset` tables, made
//! ready in leaf-to-root order via repeated `PyType_Ready` calls.
//!
//! These tests are the fast (`cargo test -p molt-lang-cpython-abi`, no wasm)
//! reproduction of the numpy `_multiarray_umath_exec` failure: the scalar
//! ArrType hierarchy (`PyGenericArrType_Type` -> `PyNumberArrType_Type` ->
//! `PyIntegerArrType_Type` -> ...) is readied with `SINGLE_INHERIT`, which sets
//! `tp_base` then calls `PyType_Ready` trusting it to (1) default a missing base
//! to `object`, (2) inherit every unset slot from the base, (3) build `tp_dict`
//! populated from `tp_methods`/`tp_getset`/`tp_members`, and (4) compute
//! `tp_mro`. A `PyType_Ready` that skips (2)-(4) leaves the derived types
//! half-initialized and later attribute access fails opaquely.

#![allow(non_snake_case)]

use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::hooks::RuntimeHooks;
use std::collections::HashMap;
use std::os::raw::{c_char, c_int};
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

// A minimal in-test runtime hook table. PyType_Ready builds `tp_dict` via
// PyDict_New and populates method names via PyUnicode_FromString; both fail
// closed (NULL) under the STUB table (alloc_dict / alloc_str return 0), so a
// stub-only test cannot exercise the readiness flow this file is about. We
// register a real-enough allocator so PyType_Ready genuinely constructs and
// populates tp_dict — the same approach test_getset_member_descriptors uses.
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(0x7200_0000);
static DICTS: Mutex<Option<HashMap<u64, HashMap<u64, u64>>>> = Mutex::new(None);
static STRINGS: Mutex<Option<HashMap<Vec<u8>, u64>>> = Mutex::new(None);

fn fresh_handle() -> u64 {
    NEXT_HANDLE.fetch_add(0x10, Ordering::Relaxed)
}

fn dicts() -> std::sync::MutexGuard<'static, Option<HashMap<u64, HashMap<u64, u64>>>> {
    let mut guard = DICTS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    guard
}

unsafe extern "C" fn fake_alloc_dict() -> u64 {
    let handle = fresh_handle();
    dicts().as_mut().unwrap().insert(handle, HashMap::new());
    handle
}

unsafe extern "C" fn fake_dict_set(dict_bits: u64, key_bits: u64, val_bits: u64) {
    if let Some(map) = dicts().as_mut().unwrap().get_mut(&dict_bits) {
        map.insert(key_bits, val_bits);
    }
}

unsafe extern "C" fn fake_dict_get(dict_bits: u64, key_bits: u64) -> u64 {
    dicts()
        .as_ref()
        .unwrap()
        .get(&dict_bits)
        .and_then(|map| map.get(&key_bits).copied())
        .unwrap_or(0)
}

unsafe extern "C" fn fake_alloc_str(data: *const u8, len: usize) -> u64 {
    let bytes = if data.is_null() {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }.to_vec()
    };
    let mut guard = STRINGS.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HashMap::new());
    }
    *guard.as_mut().unwrap().entry(bytes).or_insert_with(fresh_handle)
}

unsafe extern "C" fn fake_classify_heap(_bits: u64) -> u8 {
    MoltTypeTag::Other as u8
}

unsafe extern "C" fn fake_noop_ref(_bits: u64) {}

fn init() {
    let mut hooks: RuntimeHooks = molt_cpython_abi::hooks::STUB_HOOKS;
    hooks.alloc_dict = fake_alloc_dict;
    hooks.dict_set = fake_dict_set;
    hooks.dict_get = fake_dict_get;
    hooks.alloc_str = fake_alloc_str;
    hooks.classify_heap = fake_classify_heap;
    hooks.inc_ref = fake_noop_ref;
    hooks.dec_ref = fake_noop_ref;
    unsafe {
        molt_cpython_abi::bridge::molt_cpython_abi_init();
        let _ = molt_cpython_abi::try_set_runtime_hooks(hooks);
    }
}

unsafe fn ready(tp: *mut PyTypeObject) -> c_int {
    unsafe { molt_cpython_abi::api::typeobj::PyType_Ready(tp) }
}

/// A trivial C method used to populate a `tp_methods` table.
unsafe extern "C" fn dummy_method(_self: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
    let none = &raw mut Py_None;
    unsafe { molt_cpython_abi::api::refcount::Py_INCREF(none) };
    none
}

fn method_def(name: &'static [u8]) -> PyMethodDef {
    assert_eq!(
        *name.last().unwrap(),
        0,
        "method name must be NUL-terminated"
    );
    PyMethodDef {
        ml_name: name.as_ptr() as *const c_char,
        ml_meth: Some(dummy_method),
        ml_flags: METH_VARARGS,
        ml_doc: ptr::null(),
    }
}

/// Sentinel-terminated method table (mirrors numpy's `{NULL, NULL, 0, NULL}`).
fn method_sentinel() -> PyMethodDef {
    PyMethodDef {
        ml_name: ptr::null(),
        ml_meth: None,
        ml_flags: 0,
        ml_doc: ptr::null(),
    }
}

// ---------------------------------------------------------------------------
// (1) Missing tp_base defaults to object.
// ---------------------------------------------------------------------------

#[test]
fn ready_defaults_missing_base_to_object() {
    init();
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"root_scalar".as_ptr();
    assert!(tp.tp_base.is_null());
    let rc = unsafe { ready(&mut tp) };
    assert_eq!(
        rc, 0,
        "PyType_Ready must succeed for a base-less static type"
    );
    let object = &raw mut PyBaseObject_Type;
    assert_eq!(
        tp.tp_base, object,
        "a static type with no tp_base must inherit object as its base"
    );
    assert_ne!(tp.tp_flags & Py_TPFLAGS_READY, 0);
}

// ---------------------------------------------------------------------------
// (2) Slot inheritance down a multi-level chain (numpy SINGLE_INHERIT).
// ---------------------------------------------------------------------------

unsafe extern "C" fn base_hash(_o: *mut PyObject) -> Py_hash_t {
    42
}

type HashFn = unsafe extern "C" fn(*mut PyObject) -> Py_hash_t;

fn hash_addr(h: Option<HashFn>) -> usize {
    h.map(|f| f as HashFn as usize).unwrap_or(0)
}

fn base_hash_addr() -> usize {
    (base_hash as HashFn) as usize
}

#[test]
fn ready_inherits_slots_through_single_inherit_chain() {
    init();

    // Root: defines tp_hash and an opaque tp_as_number table; readied first.
    let mut number_methods: [u8; 256] = [0; 256];
    let number_methods_ptr = number_methods.as_mut_ptr() as *mut std::os::raw::c_void;
    let mut generic: PyTypeObject = unsafe { std::mem::zeroed() };
    generic.tp_name = c"generic".as_ptr();
    generic.tp_basicsize = 32;
    generic.tp_hash = Some(base_hash);
    generic.tp_as_number = number_methods_ptr;
    assert_eq!(unsafe { ready(&mut generic) }, 0);

    // Child: numpy sets only tp_base then readies. Everything else must inherit.
    let mut number: PyTypeObject = unsafe { std::mem::zeroed() };
    number.tp_name = c"number".as_ptr();
    number.tp_base = &mut generic;
    assert_eq!(unsafe { ready(&mut number) }, 0);
    assert_eq!(
        hash_addr(number.tp_hash),
        base_hash_addr(),
        "tp_hash must inherit"
    );
    assert_eq!(
        number.tp_as_number, number_methods_ptr,
        "tp_as_number sub-struct pointer must inherit"
    );
    assert_eq!(
        number.tp_basicsize, 32,
        "tp_basicsize must inherit from base when unset"
    );

    // Grandchild: two levels deep, still inherits the root's slots.
    let mut integer: PyTypeObject = unsafe { std::mem::zeroed() };
    integer.tp_name = c"integer".as_ptr();
    integer.tp_base = &mut number;
    assert_eq!(unsafe { ready(&mut integer) }, 0);
    assert_eq!(
        hash_addr(integer.tp_hash),
        base_hash_addr(),
        "tp_hash must inherit transitively down the chain"
    );
    assert_eq!(integer.tp_as_number, number_methods_ptr);
}

// ---------------------------------------------------------------------------
// (3) tp_dict is built and populated from tp_methods.
// ---------------------------------------------------------------------------

#[test]
fn ready_populates_tp_dict_from_methods() {
    init();
    let mut methods = [
        method_def(b"reduce\0"),
        method_def(b"item\0"),
        method_sentinel(),
    ];
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"scalar_with_methods".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    tp.tp_methods = methods.as_mut_ptr();

    assert_eq!(unsafe { ready(&mut tp) }, 0);
    // PyType_Ready must create tp_dict for a type with methods. (Retrieval of the
    // stored descriptors routes through the runtime dict bridge, so value-level
    // assertions live in the runtime-linked differential path, not this pure
    // cpython-abi unit test; here we prove the dict is created and population
    // does not error.)
    assert!(
        !tp.tp_dict.is_null(),
        "PyType_Ready must create tp_dict for a type with methods"
    );
}

// ---------------------------------------------------------------------------
// (4) tp_mro is computed for single inheritance.
// ---------------------------------------------------------------------------

#[test]
fn ready_computes_single_inheritance_mro() {
    init();
    let mut base: PyTypeObject = unsafe { std::mem::zeroed() };
    base.tp_name = c"mro_base".as_ptr();
    assert_eq!(unsafe { ready(&mut base) }, 0);

    let mut derived: PyTypeObject = unsafe { std::mem::zeroed() };
    derived.tp_name = c"mro_derived".as_ptr();
    derived.tp_base = &mut base;
    assert_eq!(unsafe { ready(&mut derived) }, 0);

    assert!(
        !derived.tp_mro.is_null(),
        "PyType_Ready must compute tp_mro"
    );
    // MRO for single inheritance is [derived, base, object]; at minimum it must
    // start with the type itself and contain the base.
    let mro = derived.tp_mro;
    let len = unsafe { molt_cpython_abi::api::sequences::PyTuple_Size(mro) };
    assert!(len >= 2, "single-inheritance MRO has at least [self, base]");
    let first = unsafe { molt_cpython_abi::api::sequences::PyTuple_GetItem(mro, 0) };
    assert_eq!(
        first,
        (&mut derived as *mut PyTypeObject).cast::<PyObject>(),
        "MRO[0] must be the type itself"
    );
}

// ---------------------------------------------------------------------------
// _PyType_Lookup walks the derived type's MRO (structure verified without the
// runtime dict bridge: the derived MRO must include the base whose tp_dict holds
// the inherited method; descriptor retrieval routes through the runtime bridge).
// ---------------------------------------------------------------------------

#[test]
fn type_lookup_walks_derived_mro_including_base() {
    init();
    let mut methods = [method_def(b"shared\0"), method_sentinel()];
    let mut base: PyTypeObject = unsafe { std::mem::zeroed() };
    base.tp_name = c"lookup_base".as_ptr();
    base.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    base.tp_methods = methods.as_mut_ptr();
    assert_eq!(unsafe { ready(&mut base) }, 0);
    assert!(!base.tp_dict.is_null(), "base must have a created tp_dict");

    let mut derived: PyTypeObject = unsafe { std::mem::zeroed() };
    derived.tp_name = c"lookup_derived".as_ptr();
    derived.tp_base = &mut base;
    assert_eq!(unsafe { ready(&mut derived) }, 0);

    // The derived MRO must contain the base type so _PyType_Lookup's MRO walk
    // reaches the base's tp_dict.
    let mro = derived.tp_mro;
    assert!(!mro.is_null());
    let n = unsafe { molt_cpython_abi::api::sequences::PyTuple_Size(mro) };
    let mut saw_base = false;
    for i in 0..n {
        let entry = unsafe { molt_cpython_abi::api::sequences::PyTuple_GetItem(mro, i) };
        if entry == (&mut base as *mut PyTypeObject).cast::<PyObject>() {
            saw_base = true;
        }
    }
    assert!(
        saw_base,
        "derived MRO must include the base so _PyType_Lookup can reach inherited methods"
    );
}

// ---------------------------------------------------------------------------
// (3) tp_free / tp_alloc defaulting — CPython's post-PyType_Ready invariant.
//
// numpy's `PyBoundArrayMethod_Type` (Py_TPFLAGS_DEFAULT, own tp_dealloc, NULL
// tp_free) ends `boundarraymethod_dealloc` with `Py_TYPE(self)->tp_free(self)`.
// CPython guarantees tp_free is non-NULL after readying (verified against
// CPython 3.12: non-GC builtins carry tp_free == PyObject_Free, GC builtins
// carry tp_free == PyObject_GC_Del, every readied type carries
// tp_alloc == PyType_GenericAlloc). A NULL tp_free turns that dealloc into a
// `call_indirect` on table index 0, which traps ("null function or function
// signature mismatch") on the first dealloc in the split wasm runtime.
// ---------------------------------------------------------------------------

type FreeFn = unsafe extern "C" fn(*mut std::ffi::c_void);
type AllocFn = unsafe extern "C" fn(*mut PyTypeObject, Py_ssize_t) -> *mut PyObject;

fn free_addr(f: Option<FreeFn>) -> usize {
    f.map(|g| g as FreeFn as usize).unwrap_or(0)
}

fn alloc_addr(f: Option<AllocFn>) -> usize {
    f.map(|g| g as AllocFn as usize).unwrap_or(0)
}

unsafe extern "C" fn dummy_dealloc(_o: *mut PyObject) {}

#[test]
fn ready_fills_tp_free_for_non_gc_type_like_bound_array_method() {
    init();
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"numpy._BoundArrayMethod".as_ptr();
    tp.tp_basicsize = 32;
    tp.tp_dealloc = Some(dummy_dealloc);
    tp.tp_flags = Py_TPFLAGS_DEFAULT; // non-GC (no Py_TPFLAGS_HAVE_GC)
    assert!(tp.tp_free.is_none(), "precondition: extension leaves tp_free NULL");
    assert_eq!(unsafe { ready(&mut tp) }, 0);
    assert_eq!(
        free_addr(tp.tp_free),
        molt_cpython_abi::api::memory::PyObject_Free as FreeFn as usize,
        "a non-GC type that leaves tp_free NULL must inherit PyObject_Free; a NULL \
         tp_free makes the extension's tp_dealloc call_indirect a null table slot"
    );
    assert_eq!(
        alloc_addr(tp.tp_alloc),
        molt_cpython_abi::api::typeobj::PyType_GenericAlloc as AllocFn as usize,
        "tp_alloc must default to PyType_GenericAlloc after readying"
    );
}

#[test]
fn ready_fills_tp_free_for_gc_type_with_gc_del() {
    init();
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"gc_scalar".as_ptr();
    tp.tp_basicsize = 32;
    tp.tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HAVE_GC;
    assert!(tp.tp_free.is_none());
    assert_eq!(unsafe { ready(&mut tp) }, 0);
    assert_eq!(
        free_addr(tp.tp_free),
        molt_cpython_abi::api::memory::PyObject_GC_Del as FreeFn as usize,
        "a GC type that leaves tp_free NULL must get PyObject_GC_Del after readying"
    );
}

#[test]
fn ready_preserves_explicit_tp_free() {
    init();
    let mut tp: PyTypeObject = unsafe { std::mem::zeroed() };
    tp.tp_name = c"custom_free".as_ptr();
    tp.tp_basicsize = 32;
    tp.tp_flags = Py_TPFLAGS_DEFAULT;
    tp.tp_free = Some(molt_cpython_abi::api::memory::PyMem_Free);
    assert_eq!(unsafe { ready(&mut tp) }, 0);
    assert_eq!(
        free_addr(tp.tp_free),
        molt_cpython_abi::api::memory::PyMem_Free as FreeFn as usize,
        "an explicit tp_free must not be overwritten by the default"
    );
}
