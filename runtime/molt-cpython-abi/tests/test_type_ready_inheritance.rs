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

// ---------------------------------------------------------------------------
// (4) type.tp_call (CPython type_call) — calling a C-extension type object.
//
// numpy's PyArrayDTypeMeta_Type sets tp_base = &PyType_Type at import time and
// its DType-class instances (BoolDType, ...) are instantiated FROM C by calling
// the class: Py_TYPE(cls)->tp_call == type.tp_call. Verified against CPython
// v3.12.13 Objects/typeobject.c::type_call. A zeroed PyType_Type.tp_call turns
// every DType() call into "'numpy._DTypeMeta' object is not callable" during
// _multiarray_umath init.
// ---------------------------------------------------------------------------

static NEW_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static INIT_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

unsafe extern "C" fn counting_new(
    tp: *mut PyTypeObject,
    _args: *mut PyObject,
    _kwds: *mut PyObject,
) -> *mut PyObject {
    NEW_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    unsafe { molt_cpython_abi::api::typeobj::PyType_GenericAlloc(tp, 0) }
}

unsafe extern "C" fn counting_init(
    _obj: *mut PyObject,
    _args: *mut PyObject,
    _kwds: *mut PyObject,
) -> c_int {
    INIT_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    0
}

#[test]
fn metatype_inherits_type_call_and_instantiates_via_tp_new_tp_init() {
    init();
    // PyType_Type carries CPython's type_call after static init.
    let type_type = &raw mut PyType_Type;
    let type_call = unsafe { (*type_type).tp_call };
    assert!(
        type_call.is_some(),
        "PyType_Type.tp_call must be CPython's type_call after init_static_types"
    );

    // A metatype like numpy's _DTypeMeta: tp_base = &PyType_Type, readied.
    let mut meta: PyTypeObject = unsafe { std::mem::zeroed() };
    meta.tp_name = c"numpy._DTypeMeta".as_ptr();
    meta.tp_basicsize = std::mem::size_of::<PyTypeObject>() as Py_ssize_t;
    meta.tp_base = type_type;
    assert_eq!(unsafe { ready(&mut meta) }, 0);
    assert!(
        meta.tp_call.is_some(),
        "a metatype with tp_base=&PyType_Type must inherit type_call via PyType_Ready"
    );

    // A "DType class": an instance of the metatype with its own tp_new/tp_init
    // (numpy's legacy_dtype_default_new pattern).
    let mut dtype_class: PyTypeObject = unsafe { std::mem::zeroed() };
    dtype_class.tp_name = c"numpy.dtypes.BoolDType".as_ptr();
    dtype_class.tp_basicsize = 64;
    dtype_class.tp_new = Some(counting_new);
    dtype_class.tp_init = Some(counting_init);
    dtype_class.ob_base.ob_base.ob_type = &mut meta;
    assert_eq!(unsafe { ready(&mut dtype_class) }, 0);

    // Calling the DType class through the inherited type_call must run
    // tp_new + tp_init and yield an instance of the class.
    let before_new = NEW_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    let before_init = INIT_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    let args = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(0) };
    assert!(!args.is_null());
    let call = meta.tp_call.expect("inherited type_call");
    let obj = unsafe {
        call(
            (&mut dtype_class as *mut PyTypeObject).cast::<PyObject>(),
            args,
            ptr::null_mut(),
        )
    };
    assert!(
        !obj.is_null(),
        "type_call must instantiate via the class's own tp_new"
    );
    assert_eq!(
        NEW_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        before_new + 1,
        "tp_new must be invoked exactly once"
    );
    assert_eq!(
        INIT_CALLS.load(std::sync::atomic::Ordering::SeqCst),
        before_init + 1,
        "tp_init must run when the result is an instance of the called type"
    );
    assert_eq!(
        unsafe { (*obj).ob_type },
        &mut dtype_class as *mut PyTypeObject,
        "the instance's ob_type must be the called class"
    );
}

#[test]
fn type_call_without_tp_new_raises_type_error_not_null_funcref() {
    init();
    let type_type = &raw mut PyType_Type;
    let call = unsafe { (*type_type).tp_call }.expect("type_call installed");
    let mut bare: PyTypeObject = unsafe { std::mem::zeroed() };
    bare.tp_name = c"bare_type".as_ptr();
    bare.tp_flags = Py_TPFLAGS_READY; // readied, but no tp_new anywhere
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let args = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(0) };
    let obj = unsafe {
        call(
            (&mut bare as *mut PyTypeObject).cast::<PyObject>(),
            args,
            ptr::null_mut(),
        )
    };
    assert!(obj.is_null(), "a type without tp_new must not instantiate");
    assert!(
        !unsafe { molt_cpython_abi::api::errors::PyErr_Occurred() }.is_null(),
        "the failure must set a TypeError, never a bare NULL"
    );
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
}

// Records what `args` its caller received: whether the pointer was NULL and, if
// not, the tuple length. Proves the CPython `PyObject_CallObject(c, NULL)`
// contract at the `tp_new` boundary.
static RECORD_ARGS_WAS_NULL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);
static RECORD_ARGS_LEN: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(-777);

unsafe extern "C" fn recording_new(
    tp: *mut PyTypeObject,
    args: *mut PyObject,
    _kwds: *mut PyObject,
) -> *mut PyObject {
    RECORD_ARGS_WAS_NULL.store(args.is_null(), std::sync::atomic::Ordering::SeqCst);
    let len = if args.is_null() {
        -1
    } else {
        unsafe { molt_cpython_abi::api::sequences::PyTuple_Size(args) as i64 }
    };
    RECORD_ARGS_LEN.store(len, std::sync::atomic::Ordering::SeqCst);
    unsafe { molt_cpython_abi::api::typeobj::PyType_GenericAlloc(tp, 0) }
}

/// Regression: `PyObject_CallObject(callable, NULL)` MUST invoke the callee's
/// `tp_call`/`tp_new` with the empty-tuple singleton, never a NULL `args`
/// pointer — the CPython contract (Objects/call.c routes NULL args through
/// `_PyObject_CallNoArgs`). numpy's `use_new_as_default` (dtypemeta.c) relies on
/// exactly this to build a parametric DType's default descriptor:
/// `PyObject_CallObject((PyObject*)DTypeClass, NULL)` and the DType's `tp_new`
/// (e.g. numpy `stringdtype_new`) parses `args` as a tuple. Forwarding NULL
/// strands that `tp_new`.
#[test]
fn call_object_with_null_args_passes_empty_tuple_not_null_to_tp_new() {
    init();
    let type_type = &raw mut PyType_Type;

    // Metatype (numpy's `_DTypeMeta` shape): tp_base = &PyType_Type, readied so
    // it inherits `type_call` as its `tp_call`.
    let mut meta: PyTypeObject = unsafe { std::mem::zeroed() };
    meta.tp_name = c"numpy._DTypeMeta".as_ptr();
    meta.tp_basicsize = std::mem::size_of::<PyTypeObject>() as Py_ssize_t;
    meta.tp_base = type_type;
    assert_eq!(unsafe { ready(&mut meta) }, 0);

    // A parametric "DType class" whose `tp_new` records its `args`.
    let mut dtype_class: PyTypeObject = unsafe { std::mem::zeroed() };
    dtype_class.tp_name = c"numpy.dtypes.StringDType".as_ptr();
    dtype_class.tp_basicsize = 64;
    dtype_class.tp_new = Some(recording_new);
    dtype_class.ob_base.ob_base.ob_type = &mut meta;
    assert_eq!(unsafe { ready(&mut dtype_class) }, 0);

    RECORD_ARGS_WAS_NULL.store(true, std::sync::atomic::Ordering::SeqCst);
    RECORD_ARGS_LEN.store(-777, std::sync::atomic::Ordering::SeqCst);

    // The exact numpy call: PyObject_CallObject(DTypeClass, NULL).
    let obj = unsafe {
        molt_cpython_abi::api::object::PyObject_CallObject(
            (&mut dtype_class as *mut PyTypeObject).cast::<PyObject>(),
            ptr::null_mut(),
        )
    };
    assert!(
        !obj.is_null(),
        "PyObject_CallObject must instantiate through tp_new"
    );
    assert!(
        !RECORD_ARGS_WAS_NULL.load(std::sync::atomic::Ordering::SeqCst),
        "tp_new must receive a real (empty-tuple) args pointer, never NULL — \
         the CPython PyObject_CallObject(c, NULL) contract"
    );
    assert_eq!(
        RECORD_ARGS_LEN.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the synthesized args must be an EMPTY tuple (len 0)"
    );
}

/// CPython's `PyObject_TypeCheck(ob, tp)` is
/// `Py_IS_TYPE(ob, tp) || PyType_IsSubtype(Py_TYPE(ob), tp)` — an instance
/// type-checks against its exact type AND against every BASE on its `tp_base`
/// chain. numpy's `PyArray_DescrCheck(res)` expands to
/// `PyObject_TypeCheck(res, &PyArrayDescr_Type)`, and a DType descriptor's
/// `Py_TYPE` is its concrete DType class (numpy `stringdtype/dtype.c` sets
/// `StringDType.tp_base = &PyArrayDescr_Type`), never `PyArrayDescr_Type`
/// itself. An EXACT-only `PyObject_TypeCheck` therefore rejects every genuine
/// descriptor and strands `use_new_as_default` (dtypemeta.c) with
/// "Instantiating <DType> did not return a dtype instance". This test asserts
/// the SUBTYPE arm and is mask-proof: it fails against an exact-only check.
#[test]
fn typecheck_matches_base_type_like_pyarray_descrcheck() {
    use molt_cpython_abi::api::typeobj::PyObject_TypeCheck;
    init();

    // Base type: numpy's `PyArrayDescr_Type` ("numpy.dtype").
    let mut descr_base: PyTypeObject = unsafe { std::mem::zeroed() };
    descr_base.tp_name = c"numpy.dtype".as_ptr();
    descr_base.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    assert_eq!(unsafe { ready(&mut descr_base) }, 0);

    // Concrete DType class: `StringDType`, whose `tp_base` is the descr base —
    // exactly numpy's `StringDType.tp_base = &PyArrayDescr_Type`.
    let mut string_dtype: PyTypeObject = unsafe { std::mem::zeroed() };
    string_dtype.tp_name = c"numpy.dtypes.StringDType".as_ptr();
    string_dtype.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    string_dtype.tp_base = &raw mut descr_base;
    assert_eq!(unsafe { ready(&mut string_dtype) }, 0);

    // An UNRELATED readied type, to prove the subtype walk does not over-match.
    let mut unrelated: PyTypeObject = unsafe { std::mem::zeroed() };
    unrelated.tp_name = c"numpy.ndarray".as_ptr();
    unrelated.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    assert_eq!(unsafe { ready(&mut unrelated) }, 0);

    // The descriptor instance numpy's `tp_new` returns: its `ob_type` is the
    // concrete DType class, exactly like the result of `StringDType()`.
    let mut descr_instance = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut string_dtype,
    };
    let inst = &raw mut descr_instance;

    // Exact-type arm (`Py_IS_TYPE`).
    assert_eq!(
        unsafe { PyObject_TypeCheck(inst, &raw mut string_dtype) },
        1,
        "an instance must type-check against its exact type",
    );
    // Subtype arm (`PyType_IsSubtype`) — the numpy `PyArray_DescrCheck(res)`
    // case and the teeth of this regression: an exact-only check returns 0.
    assert_eq!(
        unsafe { PyObject_TypeCheck(inst, &raw mut descr_base) },
        1,
        "a DType descriptor must type-check against its base PyArrayDescr_Type \
         (PyObject_TypeCheck = Py_IS_TYPE || PyType_IsSubtype); exact-only \
         stranded numpy use_new_as_default",
    );
    // Must NOT match an unrelated type: the walk terminates at `object`.
    assert_eq!(
        unsafe { PyObject_TypeCheck(inst, &raw mut unrelated) },
        0,
        "the subtype walk must not over-match an unrelated type",
    );
}
