//! Native readiness uses the production runtime's dictionary, descriptor and GC
//! owners. These declarations reproduce extension single-inheritance, namespace,
//! metaclass-call and attribute-slot contracts without a second runtime fixture.

#![allow(non_snake_case)]

use super::super::native_test_fixture::NativeType;
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{mapping, refcount, strings, typeobj};
use std::ffi::c_void;
use std::os::raw::{c_char, c_int};
use std::ptr;

mod method_descriptors;

struct TypeReadyTestTransaction {
    // Release execution admission, thread state and GIL before restoring the transaction.
    _execution: crate::concurrency::RuntimeExecutionGuard,
    _runtime: crate::test_support::RuntimeTestTransaction,
}

fn init() -> TypeReadyTestTransaction {
    let runtime = crate::test_support::RuntimeTestTransaction::new();
    TypeReadyTestTransaction {
        _execution: crate::concurrency::RuntimeExecutionGuard::enter(),
        _runtime: runtime,
    }
}

unsafe fn ready(tp: *mut PyTypeObject) -> c_int {
    unsafe { typeobj::PyType_Ready(tp) }
}

fn dict_value_by_name(dict: *mut PyObject, name: &[u8]) -> *mut PyObject {
    unsafe {
        let key =
            strings::PyUnicode_FromStringAndSize(name.as_ptr().cast(), name.len() as Py_ssize_t);
        assert!(!key.is_null());
        let value = mapping::PyDict_GetItemWithError(dict, key);
        refcount::Py_DECREF(key);
        assert!(!value.is_null(), "declared name must be present in tp_dict");
        value
    }
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
    let _guard = init();
    let mut tp = NativeType::<PyTypeObject>::new();
    tp.tp_name = c"root_scalar".as_ptr();
    assert!(tp.tp_base.is_null());
    let rc = unsafe { ready(&mut *tp) };
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
    let _guard = init();

    // Root: defines tp_hash and an opaque tp_as_number table; readied first.
    let mut number_methods: [u8; 256] = [0; 256];
    let number_methods_ptr = number_methods.as_mut_ptr() as *mut std::os::raw::c_void;
    let mut generic = NativeType::<PyTypeObject>::new();
    generic.tp_name = c"generic".as_ptr();
    generic.tp_basicsize = 32;
    generic.tp_hash = Some(base_hash);
    generic.tp_as_number = number_methods_ptr;
    assert_eq!(unsafe { ready(&mut *generic) }, 0);

    // Child: numpy sets only tp_base then readies. Everything else must inherit.
    let mut number = NativeType::<PyTypeObject>::new();
    number.tp_name = c"number".as_ptr();
    number.tp_base = &mut *generic;
    assert_eq!(unsafe { ready(&mut *number) }, 0);
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
    let mut integer = NativeType::<PyTypeObject>::new();
    integer.tp_name = c"integer".as_ptr();
    integer.tp_base = &mut *number;
    assert_eq!(unsafe { ready(&mut *integer) }, 0);
    assert_eq!(
        hash_addr(integer.tp_hash),
        base_hash_addr(),
        "tp_hash must inherit transitively down the chain"
    );
    assert_eq!(integer.tp_as_number, number_methods_ptr);
}

unsafe extern "C" fn slot_repr(_o: *mut PyObject) -> *mut PyObject {
    &raw mut Py_None
}

unsafe extern "C" fn slot_richcompare(
    _left: *mut PyObject,
    _right: *mut PyObject,
    _op: c_int,
) -> *mut PyObject {
    &raw mut Py_NotImplementedSentinel
}

unsafe extern "C" fn slot_call(
    _callable: *mut PyObject,
    _args: *mut PyObject,
    _kwargs: *mut PyObject,
) -> *mut PyObject {
    &raw mut Py_None
}

unsafe extern "C" fn slot_marker() {}

#[test]
fn ready_synthesizes_slot_wrapper_dunders_and_unhashable_none() {
    let _guard = init();
    let mut sequence: PySequenceMethods = unsafe { std::mem::zeroed() };
    let marker = slot_marker as *const () as *mut std::ffi::c_void;
    sequence.sq_length = marker;
    sequence.sq_item = marker;
    sequence.sq_contains = marker;

    let mut base = NativeType::<PyTypeObject>::new();
    base.tp_name = c"slot_wrapper_base".as_ptr();
    base.tp_hash = Some(base_hash);
    base.tp_repr = Some(slot_repr);
    base.tp_call = Some(slot_call);
    base.tp_richcompare = Some(slot_richcompare);
    base.tp_as_sequence = (&mut sequence as *mut PySequenceMethods).cast();
    assert_eq!(unsafe { ready(&mut *base) }, 0);

    let mut derived = NativeType::<PyTypeObject>::new();
    derived.tp_name = c"slot_wrapper_derived".as_ptr();
    derived.tp_base = &mut *base;
    assert_eq!(unsafe { ready(&mut *derived) }, 0);

    for name in [
        c"__hash__",
        c"__repr__",
        c"__call__",
        c"__eq__",
        c"__ne__",
        c"__lt__",
        c"__le__",
        c"__gt__",
        c"__ge__",
        c"__len__",
        c"__getitem__",
        c"__contains__",
    ] {
        let name_bytes = name.to_bytes();
        let value = dict_value_by_name(base.tp_dict, name_bytes);
        unsafe {
            let inherited = mapping::_PyDict_GetItemStringWithError(derived.tp_dict, name.as_ptr());
            assert!(
                inherited.is_null(),
                "inherited wrappers do not become declarations"
            );
            assert!(molt_cpython_abi::api::errors::PyErr_Occurred().is_null());
            let key = strings::PyUnicode_FromString(name.as_ptr());
            assert_eq!(typeobj::_PyType_Lookup(&mut *derived, key), value);
            refcount::Py_DECREF(key);
        }
        assert!(
            !value.is_null(),
            "{} must be synthesized",
            name.to_string_lossy()
        );
        assert_eq!(
            unsafe { (*value).ob_type },
            &raw mut PyWrapperDescr_Type,
            "{} must be a wrapper_descriptor",
            name.to_string_lossy()
        );
    }

    let mut instance = PyObject {
        ob_refcnt: 1,
        ob_type: &mut *derived,
    };
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyObject_Hash(&mut instance) },
        42
    );

    let mut unhashable = NativeType::<PyTypeObject>::new();
    unhashable.tp_name = c"unhashable".as_ptr();
    unhashable.tp_hash = Some(molt_cpython_abi::api::typeobj::PyObject_HashNotImplemented);
    assert_eq!(unsafe { ready(&mut *unhashable) }, 0);
    let hash_attr = dict_value_by_name(unhashable.tp_dict, b"__hash__");
    assert_eq!(
        hash_attr, &raw mut Py_None,
        "unhashable __hash__ must be None"
    );
}

// ---------------------------------------------------------------------------
// (3) tp_dict is built and populated from tp_methods.
// ---------------------------------------------------------------------------

#[test]
fn ready_populates_tp_dict_from_methods() {
    let _guard = init();
    let mut methods = [
        method_def(b"reduce\0"),
        method_def(b"item\0"),
        method_sentinel(),
    ];
    let mut tp = NativeType::<PyTypeObject>::new();
    tp.tp_name = c"scalar_with_methods".as_ptr();
    tp.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    tp.tp_methods = methods.as_mut_ptr();

    assert_eq!(unsafe { ready(&mut *tp) }, 0);
    assert!(
        !tp.tp_dict.is_null(),
        "PyType_Ready must create tp_dict for a type with methods"
    );
    for name in [b"reduce".as_slice(), b"item"] {
        let method = dict_value_by_name(tp.tp_dict, name);
        assert_eq!(unsafe { typeobj::PyCallable_Check(method) }, 1);
    }
}

// ---------------------------------------------------------------------------
// (4) tp_mro is computed for single inheritance.
// ---------------------------------------------------------------------------

#[test]
fn ready_computes_single_inheritance_mro() {
    let _guard = init();
    let mut base = NativeType::<PyTypeObject>::new();
    base.tp_name = c"mro_base".as_ptr();
    assert_eq!(unsafe { ready(&mut *base) }, 0);

    let mut derived = NativeType::<PyTypeObject>::new();
    derived.tp_name = c"mro_derived".as_ptr();
    derived.tp_base = &mut *base;
    assert_eq!(unsafe { ready(&mut *derived) }, 0);

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
        (&mut *derived as *mut PyTypeObject).cast::<PyObject>(),
        "MRO[0] must be the type itself"
    );
}

// ---------------------------------------------------------------------------
// _PyType_Lookup walks the derived type's MRO and returns its inherited method.
// ---------------------------------------------------------------------------

#[test]
fn type_lookup_walks_derived_mro_including_base() {
    let _guard = init();
    let mut methods = [method_def(b"shared\0"), method_sentinel()];
    let mut base = NativeType::<PyTypeObject>::new();
    base.tp_name = c"lookup_base".as_ptr();
    base.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    base.tp_methods = methods.as_mut_ptr();
    assert_eq!(unsafe { ready(&mut *base) }, 0);
    assert!(!base.tp_dict.is_null(), "base must have a created tp_dict");

    let mut derived = NativeType::<PyTypeObject>::new();
    derived.tp_name = c"lookup_derived".as_ptr();
    derived.tp_base = &mut *base;
    assert_eq!(unsafe { ready(&mut *derived) }, 0);

    // The derived MRO must contain the base type so _PyType_Lookup's MRO walk
    // reaches the base's tp_dict.
    let mro = derived.tp_mro;
    assert!(!mro.is_null());
    let n = unsafe { molt_cpython_abi::api::sequences::PyTuple_Size(mro) };
    let mut saw_base = false;
    for i in 0..n {
        let entry = unsafe { molt_cpython_abi::api::sequences::PyTuple_GetItem(mro, i) };
        if entry == (&mut *base as *mut PyTypeObject).cast::<PyObject>() {
            saw_base = true;
        }
    }
    assert!(
        saw_base,
        "derived MRO must include the base so _PyType_Lookup can reach inherited methods"
    );
    unsafe {
        let name = strings::PyUnicode_FromString(c"shared".as_ptr());
        assert!(!name.is_null());
        let inherited = typeobj::_PyType_Lookup(&mut *derived, name);
        assert_eq!(
            inherited,
            mapping::PyDict_GetItemWithError(base.tp_dict, name)
        );
        assert!(!inherited.is_null());
        refcount::Py_DECREF(name);
    }
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
    let _guard = init();
    let mut tp = NativeType::<PyTypeObject>::new();
    tp.tp_name = c"numpy._BoundArrayMethod".as_ptr();
    tp.tp_basicsize = 32;
    tp.tp_dealloc = Some(dummy_dealloc);
    tp.tp_flags = Py_TPFLAGS_DEFAULT; // non-GC (no Py_TPFLAGS_HAVE_GC)
    assert!(
        tp.tp_free.is_none(),
        "precondition: extension leaves tp_free NULL"
    );
    assert_eq!(unsafe { ready(&mut *tp) }, 0);
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

unsafe extern "C" fn empty_traverse(
    _object: *mut PyObject,
    _visit: *mut c_void,
    _arg: *mut c_void,
) -> c_int {
    0
}

#[test]
fn ready_fills_tp_free_for_gc_type_with_gc_del() {
    let _guard = init();
    let mut tp = NativeType::<PyTypeObject>::new();
    tp.tp_name = c"gc_scalar".as_ptr();
    tp.tp_basicsize = 32;
    tp.tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HAVE_GC;
    tp.tp_traverse = Some(empty_traverse);
    assert!(tp.tp_free.is_none());
    assert_eq!(unsafe { ready(&mut *tp) }, 0);
    assert_eq!(
        free_addr(tp.tp_free),
        molt_cpython_abi::api::memory::PyObject_GC_Del as FreeFn as usize,
        "a GC type that leaves tp_free NULL must get PyObject_GC_Del after readying"
    );
}

#[test]
fn ready_preserves_explicit_tp_free() {
    let _guard = init();
    let mut tp = NativeType::<PyTypeObject>::new();
    tp.tp_name = c"custom_free".as_ptr();
    tp.tp_basicsize = 32;
    tp.tp_flags = Py_TPFLAGS_DEFAULT;
    tp.tp_free = Some(molt_cpython_abi::api::memory::PyMem_Free);
    assert_eq!(unsafe { ready(&mut *tp) }, 0);
    assert_eq!(
        free_addr(tp.tp_free),
        molt_cpython_abi::api::memory::PyMem_Free as FreeFn as usize,
        "an explicit tp_free must not be overwritten by the default"
    );
}

// ---------------------------------------------------------------------------
// (3b) type.tp_is_gc — the canonical metatype GC predicate.
//
// numpy's `dtypemeta_is_gc` reads `PyType_Type.tp_is_gc` directly and invokes
// it as an indirect function pointer.  CPython 3.12's `type_is_gc` returns the
// candidate type object's HEAPTYPE bit.  A NULL canonical slot is not an
// optional feature in this path: wasm dispatches table entry zero and traps.
// ---------------------------------------------------------------------------

type IsGcFn = unsafe extern "C" fn(*mut PyObject) -> c_int;

fn is_gc_addr(f: Option<IsGcFn>) -> usize {
    f.map(|g| g as IsGcFn as usize).unwrap_or(0)
}

#[test]
fn type_is_gc_matches_cpython_heaptype_predicate() {
    let _guard = init();
    let type_type = &raw mut PyType_Type;
    let is_gc = unsafe { (*type_type).tp_is_gc }
        .expect("PyType_Type.tp_is_gc must publish a callable table entry");

    let mut static_type = NativeType::<PyTypeObject>::new();
    static_type.tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HAVE_GC;
    assert_eq!(
        unsafe { is_gc((&mut *static_type as *mut PyTypeObject).cast()) },
        0,
        "HAVE_GC alone does not make a static type object GC-tracked"
    );

    let mut heap_type = NativeType::<PyTypeObject>::new();
    heap_type.tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HEAPTYPE;
    assert_eq!(
        unsafe { is_gc((&mut *heap_type as *mut PyTypeObject).cast()) },
        Py_TPFLAGS_HEAPTYPE as c_int,
        "CPython type_is_gc returns the HEAPTYPE bit without normalization"
    );
    assert_eq!(unsafe { is_gc(ptr::null_mut()) }, 0);
}

#[test]
fn metatype_inherits_canonical_type_is_gc_callable() {
    let _guard = init();
    let type_type = &raw mut PyType_Type;
    let canonical = unsafe { (*type_type).tp_is_gc };
    assert_ne!(
        is_gc_addr(canonical),
        0,
        "PyType_Type.tp_is_gc must never encode the null wasm table slot"
    );

    let mut meta = NativeType::<PyTypeObject>::new();
    meta.tp_name = c"numpy._DTypeMeta".as_ptr();
    meta.tp_basicsize = std::mem::size_of::<PyTypeObject>() as Py_ssize_t;
    meta.tp_base = type_type;
    assert_eq!(unsafe { ready(&mut *meta) }, 0);
    assert_eq!(
        is_gc_addr(meta.tp_is_gc),
        is_gc_addr(canonical),
        "a metatype based on PyType_Type must inherit the canonical tp_is_gc callable"
    );
    assert_eq!(meta.tp_traverse.map(|f| f as usize), unsafe {
        (*type_type).tp_traverse.map(|f| f as usize)
    });
    assert_eq!(meta.tp_clear.map(|f| f as usize), unsafe {
        (*type_type).tp_clear.map(|f| f as usize)
    });
}

unsafe extern "C" fn collect_type_reference(op: *mut PyObject, arg: *mut c_void) -> c_int {
    let visited = unsafe { &mut *arg.cast::<Vec<usize>>() };
    visited.push(op as usize);
    0
}

#[test]
fn type_traverse_visits_exact_heap_type_ownership_family() {
    let _guard = init();
    let mut heap = NativeType::<PyHeapTypeObject>::new();
    heap.ht_type.tp_flags = Py_TPFLAGS_HEAPTYPE | Py_TPFLAGS_HAVE_GC;
    let mut references: [PyObject; 6] = unsafe { std::mem::zeroed() };
    let mut acyclic_heap_references: [PyObject; 4] = unsafe { std::mem::zeroed() };
    heap.ht_type.tp_dict = &raw mut references[0];
    heap.ht_type.tp_cache = &raw mut references[1];
    heap.ht_type.tp_mro = &raw mut references[2];
    heap.ht_type.tp_bases = &raw mut references[3];
    heap.ht_type.tp_base = (&raw mut references[4]).cast::<PyTypeObject>();
    heap.ht_module = &raw mut references[5];
    heap.ht_name = &raw mut acyclic_heap_references[0];
    heap.ht_slots = &raw mut acyclic_heap_references[1];
    heap.ht_qualname = &raw mut acyclic_heap_references[2];
    heap._spec_cache.getitem = &raw mut acyclic_heap_references[3];
    let mut visited: Vec<usize> = Vec::new();
    let traverse = unsafe { PyType_Type.tp_traverse }.expect("type_traverse installed");
    assert_eq!(
        unsafe {
            traverse(
                (&raw mut heap.ht_type).cast(),
                collect_type_reference as *mut c_void,
                (&raw mut visited).cast(),
            )
        },
        0
    );
    assert_eq!(
        visited,
        references
            .iter_mut()
            .map(|reference| reference as *mut PyObject as usize)
            .collect::<Vec<_>>()
    );
    for reference in &mut acyclic_heap_references {
        assert!(
            !visited.contains(&(reference as *mut PyObject as usize)),
            "CPython 3.12 does not visit acyclic name/slots/qualname strings or the non-owning specialization cache"
        );
    }
    // These sentinels model borrowed traversal addresses, not actual owners.
    heap.ht_type.tp_dict = ptr::null_mut();
    heap.ht_type.tp_cache = ptr::null_mut();
    heap.ht_type.tp_mro = ptr::null_mut();
    heap.ht_type.tp_bases = ptr::null_mut();
    heap.ht_type.tp_base = ptr::null_mut();
    heap.ht_module = ptr::null_mut();
    heap.ht_name = ptr::null_mut();
    heap.ht_slots = ptr::null_mut();
    heap.ht_qualname = ptr::null_mut();
    heap._spec_cache.getitem = ptr::null_mut();
}

#[test]
fn type_clear_breaks_heap_type_mro_and_module_cycles() {
    let _guard = init();
    let mut heap = NativeType::<PyHeapTypeObject>::new();
    heap.ht_type.tp_flags = Py_TPFLAGS_HEAPTYPE | Py_TPFLAGS_HAVE_GC | Py_TPFLAGS_VALID_VERSION_TAG;
    heap.ht_type.tp_version_tag = 77;
    let mut mro: PyObject = unsafe { std::mem::zeroed() };
    let mut module: PyObject = unsafe { std::mem::zeroed() };
    let mut retained: [PyObject; 6] = unsafe { std::mem::zeroed() };
    mro.ob_refcnt = 2;
    module.ob_refcnt = 2;
    heap.ht_type.tp_mro = &raw mut mro;
    heap.ht_module = &raw mut module;
    heap.ht_name = &raw mut retained[0];
    heap.ht_slots = &raw mut retained[1];
    heap.ht_qualname = &raw mut retained[2];
    heap.ht_type.tp_cache = &raw mut retained[3];
    heap.ht_type.tp_bases = &raw mut retained[4];
    heap.ht_type.tp_base = (&raw mut retained[5]).cast::<PyTypeObject>();
    heap._spec_cache.getitem = &raw mut retained[0];
    let dict = unsafe { molt_cpython_abi::api::mapping::PyDict_New() };
    assert!(!dict.is_null());
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::mapping::PyDict_SetItemString(
                dict,
                c"owned".as_ptr(),
                (&raw mut Py_None).cast(),
            )
        },
        0
    );
    assert_eq!(
        unsafe { molt_cpython_abi::api::mapping::PyDict_Size(dict) },
        1
    );
    heap.ht_type.tp_dict = dict;
    let clear = unsafe { PyType_Type.tp_clear }.expect("type_clear installed");
    assert_eq!(unsafe { clear((&raw mut heap.ht_type).cast()) }, 0);
    assert!(heap.ht_type.tp_mro.is_null());
    assert!(heap.ht_module.is_null());
    assert_eq!(
        unsafe { molt_cpython_abi::api::mapping::PyDict_Size(dict) },
        0
    );
    assert_eq!(heap.ht_type.tp_dict, dict);
    assert_eq!(heap.ht_name, &raw mut retained[0]);
    assert_eq!(heap.ht_slots, &raw mut retained[1]);
    assert_eq!(heap.ht_qualname, &raw mut retained[2]);
    assert_eq!(heap.ht_type.tp_cache, &raw mut retained[3]);
    assert_eq!(heap.ht_type.tp_bases, &raw mut retained[4]);
    assert_eq!(heap.ht_type.tp_base, (&raw mut retained[5]).cast());
    assert!(heap._spec_cache.getitem.is_null());
    assert_eq!(heap.ht_type.tp_version_tag, 0);
    assert_eq!(heap.ht_type.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
    assert_eq!(mro.ob_refcnt, 1);
    assert_eq!(module.ob_refcnt, 1);
    unsafe { refcount::Py_CLEAR(&raw mut heap.ht_type.tp_dict) };
    heap.ht_type.tp_cache = ptr::null_mut();
    heap.ht_type.tp_bases = ptr::null_mut();
    heap.ht_type.tp_base = ptr::null_mut();
    heap.ht_name = ptr::null_mut();
    heap.ht_slots = ptr::null_mut();
    heap.ht_qualname = ptr::null_mut();
}

static WATCH_CALLBACKS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static WATCH_CHILD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static CHILD_INVALID_BEFORE_BASE_CALLBACK: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

unsafe extern "C" fn type_watch_callback(type_: *mut PyObject) -> c_int {
    WATCH_CALLBACKS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let child = WATCH_CHILD.load(std::sync::atomic::Ordering::SeqCst) as *mut PyTypeObject;
    if !child.is_null()
        && unsafe { (*child).tp_flags } & Py_TPFLAGS_VALID_VERSION_TAG == 0
        && unsafe { (*type_.cast::<PyTypeObject>()).tp_flags } & Py_TPFLAGS_VALID_VERSION_TAG != 0
    {
        CHILD_INVALID_BEFORE_BASE_CALLBACK.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    0
}

#[test]
fn type_modified_recurses_subclasses_and_notifies_312_watchers() {
    let _guard = init();
    WATCH_CALLBACKS.store(0, std::sync::atomic::Ordering::SeqCst);
    CHILD_INVALID_BEFORE_BASE_CALLBACK.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut base = NativeType::<PyHeapTypeObject>::new();
    let mut child = NativeType::<PyHeapTypeObject>::new();
    for (index, heap) in [&mut *base, &mut *child].into_iter().enumerate() {
        heap.ht_type.tp_name = [c"WatchedBase", c"WatchedChild"][index].as_ptr();
        heap.ht_type.ob_base.ob_base.ob_type = &raw mut PyType_Type;
        heap.ht_type.tp_flags =
            Py_TPFLAGS_HEAPTYPE | Py_TPFLAGS_READY | Py_TPFLAGS_VALID_VERSION_TAG;
        heap.ht_type.tp_version_tag = 100 + u32::try_from(index).unwrap();
        heap._spec_cache.getitem = (&raw mut heap.ht_type).cast();
    }
    child.ht_type.tp_base = &raw mut base.ht_type;
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyType_Ready(&raw mut child.ht_type) },
        0
    );
    WATCH_CHILD.store(
        (&raw mut child.ht_type) as usize,
        std::sync::atomic::Ordering::SeqCst,
    );
    let watcher =
        unsafe { molt_cpython_abi::api::typeobj::PyType_AddWatcher(Some(type_watch_callback)) };
    assert!(watcher >= 0);
    assert_eq!(
        unsafe {
            molt_cpython_abi::api::typeobj::PyType_Watch(watcher, (&raw mut base.ht_type).cast())
        },
        0
    );
    unsafe { molt_cpython_abi::api::typeobj::PyType_Modified(&raw mut base.ht_type) };
    assert_eq!(WATCH_CALLBACKS.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(CHILD_INVALID_BEFORE_BASE_CALLBACK.load(std::sync::atomic::Ordering::SeqCst));
    for heap in [&base, &child] {
        assert_eq!(heap.ht_type.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
        assert_eq!(heap.ht_type.tp_version_tag, 0);
        assert!(heap._spec_cache.getitem.is_null());
    }
    assert_eq!(
        unsafe { molt_cpython_abi::api::typeobj::PyType_ClearWatcher(watcher) },
        0
    );
    WATCH_CHILD.store(0, std::sync::atomic::Ordering::SeqCst);
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
    let _guard = init();
    // PyType_Type carries CPython's type_call after static init.
    let type_type = &raw mut PyType_Type;
    let type_call = unsafe { (*type_type).tp_call };
    assert!(
        type_call.is_some(),
        "PyType_Type.tp_call must be CPython's type_call after init_static_types"
    );

    // A metatype like numpy's _DTypeMeta: tp_base = &PyType_Type, readied.
    let mut meta = NativeType::<PyTypeObject>::new();
    meta.tp_name = c"numpy._DTypeMeta".as_ptr();
    meta.tp_basicsize = std::mem::size_of::<PyTypeObject>() as Py_ssize_t;
    meta.tp_base = type_type;
    assert_eq!(unsafe { ready(&mut *meta) }, 0);
    assert!(
        meta.tp_call.is_some(),
        "a metatype with tp_base=&PyType_Type must inherit type_call via PyType_Ready"
    );

    // A "DType class": an instance of the metatype with its own tp_new/tp_init
    // (numpy's legacy_dtype_default_new pattern).
    let mut dtype_class = NativeType::<PyTypeObject>::new();
    dtype_class.tp_name = c"numpy.dtypes.BoolDType".as_ptr();
    dtype_class.tp_basicsize = 64;
    dtype_class.tp_new = Some(counting_new);
    dtype_class.tp_init = Some(counting_init);
    dtype_class.ob_base.ob_base.ob_type = &mut *meta;
    assert_eq!(unsafe { ready(&mut *dtype_class) }, 0);

    // Calling the DType class through the inherited type_call must run
    // tp_new + tp_init and yield an instance of the class.
    let before_new = NEW_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    let before_init = INIT_CALLS.load(std::sync::atomic::Ordering::SeqCst);
    let args = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(0) };
    assert!(!args.is_null());
    let call = meta.tp_call.expect("inherited type_call");
    let obj = unsafe {
        call(
            (&mut *dtype_class as *mut PyTypeObject).cast::<PyObject>(),
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
        &mut *dtype_class as *mut PyTypeObject,
        "the instance's ob_type must be the called class"
    );
    unsafe {
        molt_cpython_abi::api::refcount::Py_DECREF(obj);
        molt_cpython_abi::api::refcount::Py_DECREF(args);
    }
}

#[test]
fn unresolved_type_base_fails_without_repairing_metaclass_callability() {
    let _guard = init();
    let mut unresolved = NativeType::<PyTypeObject>::new();
    unresolved.tp_flags = Py_TPFLAGS_READY;
    let mut meta = NativeType::<PyTypeObject>::new();
    meta.tp_name = c"unresolved_meta".as_ptr();
    meta.tp_basicsize = std::mem::size_of::<PyTypeObject>() as Py_ssize_t;
    meta.tp_base = &mut *unresolved;
    assert_eq!(unsafe { ready(&mut *meta) }, -1);
    assert_eq!(meta.tp_flags & (Py_TPFLAGS_READY | Py_TPFLAGS_READYING), 0);
    assert!(meta.tp_call.is_none());
    assert!(unresolved.tp_call.is_none());
    unsafe {
        assert_eq!(
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut PyExc_SystemError).cast()
            ),
            1
        );
        molt_cpython_abi::api::errors::PyErr_Clear();
    }
}

#[test]
fn type_call_without_tp_new_raises_type_error_not_null_funcref() {
    let _guard = init();
    let type_type = &raw mut PyType_Type;
    let call = unsafe { (*type_type).tp_call }.expect("type_call installed");
    let mut bare = NativeType::<PyTypeObject>::new();
    bare.tp_name = c"bare_type".as_ptr();
    bare.tp_flags = Py_TPFLAGS_READY; // readied, but no tp_new anywhere
    unsafe { molt_cpython_abi::api::errors::PyErr_Clear() };
    let args = unsafe { molt_cpython_abi::api::sequences::PyTuple_New(0) };
    let obj = unsafe {
        call(
            (&mut *bare as *mut PyTypeObject).cast::<PyObject>(),
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
    unsafe { molt_cpython_abi::api::refcount::Py_DECREF(args) };
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
    let _guard = init();
    let type_type = &raw mut PyType_Type;

    // Metatype (numpy's `_DTypeMeta` shape): tp_base = &PyType_Type, readied so
    // it inherits `type_call` as its `tp_call`.
    let mut meta = NativeType::<PyTypeObject>::new();
    meta.tp_name = c"numpy._DTypeMeta".as_ptr();
    meta.tp_basicsize = std::mem::size_of::<PyTypeObject>() as Py_ssize_t;
    meta.tp_base = type_type;
    assert_eq!(unsafe { ready(&mut *meta) }, 0);

    // A parametric "DType class" whose `tp_new` records its `args`.
    let mut dtype_class = NativeType::<PyTypeObject>::new();
    dtype_class.tp_name = c"numpy.dtypes.StringDType".as_ptr();
    dtype_class.tp_basicsize = 64;
    dtype_class.tp_new = Some(recording_new);
    dtype_class.ob_base.ob_base.ob_type = &mut *meta;
    assert_eq!(unsafe { ready(&mut *dtype_class) }, 0);

    RECORD_ARGS_WAS_NULL.store(true, std::sync::atomic::Ordering::SeqCst);
    RECORD_ARGS_LEN.store(-777, std::sync::atomic::Ordering::SeqCst);

    // The exact numpy call: PyObject_CallObject(DTypeClass, NULL).
    let obj = unsafe {
        molt_cpython_abi::api::object::PyObject_CallObject(
            (&mut *dtype_class as *mut PyTypeObject).cast::<PyObject>(),
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
    let _guard = init();

    // Base type: numpy's `PyArrayDescr_Type` ("numpy.dtype").
    let mut descr_base = NativeType::<PyTypeObject>::new();
    descr_base.tp_name = c"numpy.dtype".as_ptr();
    descr_base.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    assert_eq!(unsafe { ready(&mut *descr_base) }, 0);

    // Concrete DType class: `StringDType`, whose `tp_base` is the descr base —
    // exactly numpy's `StringDType.tp_base = &PyArrayDescr_Type`.
    let mut string_dtype = NativeType::<PyTypeObject>::new();
    string_dtype.tp_name = c"numpy.dtypes.StringDType".as_ptr();
    string_dtype.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    string_dtype.tp_base = &raw mut *descr_base;
    assert_eq!(unsafe { ready(&mut *string_dtype) }, 0);

    // An UNRELATED readied type, to prove the subtype walk does not over-match.
    let mut unrelated = NativeType::<PyTypeObject>::new();
    unrelated.tp_name = c"numpy.ndarray".as_ptr();
    unrelated.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    assert_eq!(unsafe { ready(&mut *unrelated) }, 0);

    // The descriptor instance numpy's `tp_new` returns: its `ob_type` is the
    // concrete DType class, exactly like the result of `StringDType()`.
    let mut descr_instance = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *string_dtype,
    };
    let inst = &raw mut descr_instance;

    // Exact-type arm (`Py_IS_TYPE`).
    assert_eq!(
        unsafe { PyObject_TypeCheck(inst, &raw mut *string_dtype) },
        1,
        "an instance must type-check against its exact type",
    );
    // Subtype arm (`PyType_IsSubtype`) — the numpy `PyArray_DescrCheck(res)`
    // case and the teeth of this regression: an exact-only check returns 0.
    assert_eq!(
        unsafe { PyObject_TypeCheck(inst, &raw mut *descr_base) },
        1,
        "a DType descriptor must type-check against its base PyArrayDescr_Type \
         (PyObject_TypeCheck = Py_IS_TYPE || PyType_IsSubtype); exact-only \
         stranded numpy use_new_as_default",
    );
    // Must NOT match an unrelated type: the walk terminates at `object`.
    assert_eq!(
        unsafe { PyObject_TypeCheck(inst, &raw mut *unrelated) },
        0,
        "the subtype walk must not over-match an unrelated type",
    );
}

/// `PyObject_IsInstance(inst, cls)` for a type `cls` reduces to
/// `PyObject_TypeCheck(inst, cls)` (CPython `recursive_isinstance`). It is the
/// same exact-OR-subtype relationship — numpy's `descriptor.c` calls
/// `PyObject_IsInstance(conv, &PyArray_StringDType)` on descriptors whose type
/// is a subclass of the queried DType. An exact-only check returns a false
/// negative; this asserts the SUBTYPE arm and is mask-proof.
#[test]
fn isinstance_matches_base_type_via_subtype_walk() {
    use molt_cpython_abi::api::typeobj::PyObject_IsInstance;
    let _guard = init();

    let mut base = NativeType::<PyTypeObject>::new();
    base.tp_name = c"numpy.dtype".as_ptr();
    base.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    assert_eq!(unsafe { ready(&mut *base) }, 0);

    let mut derived = NativeType::<PyTypeObject>::new();
    derived.tp_name = c"numpy.dtypes.StringDType".as_ptr();
    derived.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    derived.tp_base = &raw mut *base;
    assert_eq!(unsafe { ready(&mut *derived) }, 0);

    let mut unrelated = NativeType::<PyTypeObject>::new();
    unrelated.tp_name = c"numpy.ndarray".as_ptr();
    unrelated.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    assert_eq!(unsafe { ready(&mut *unrelated) }, 0);

    let mut instance = PyObject {
        ob_refcnt: 1,
        ob_type: &raw mut *derived,
    };
    let inst = &raw mut instance;

    assert_eq!(
        unsafe { PyObject_IsInstance(inst, (&raw mut *derived).cast::<PyObject>()) },
        1,
        "isinstance against the exact class",
    );
    assert_eq!(
        unsafe { PyObject_IsInstance(inst, (&raw mut *base).cast::<PyObject>()) },
        1,
        "isinstance against a BASE class must walk the subtype chain (mask-proof)",
    );
    assert_eq!(
        unsafe { PyObject_IsInstance(inst, (&raw mut *unrelated).cast::<PyObject>()) },
        0,
        "isinstance must not over-match an unrelated class",
    );
}

/// `PyObject_IsSubclass(derived, cls)` for type args reduces to
/// `PyType_IsSubtype(derived, cls)` (CPython `recursive_issubclass`). A bare
/// pointer-identity check dropped every genuine base/derived relationship; this
/// asserts the SUBTYPE arm (mask-proof) plus the exact and no-over-match cases,
/// and that the relationship is DIRECTIONAL (base is not a subclass of derived).
#[test]
fn issubclass_walks_base_chain() {
    use molt_cpython_abi::api::object::PyObject_IsSubclass;
    let _guard = init();

    let mut base = NativeType::<PyTypeObject>::new();
    base.tp_name = c"numpy.dtype".as_ptr();
    base.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    assert_eq!(unsafe { ready(&mut *base) }, 0);

    let mut derived = NativeType::<PyTypeObject>::new();
    derived.tp_name = c"numpy.dtypes.StringDType".as_ptr();
    derived.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    derived.tp_base = &raw mut *base;
    assert_eq!(unsafe { ready(&mut *derived) }, 0);

    let mut unrelated = NativeType::<PyTypeObject>::new();
    unrelated.tp_name = c"numpy.ndarray".as_ptr();
    unrelated.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    assert_eq!(unsafe { ready(&mut *unrelated) }, 0);

    let base_o = (&raw mut *base).cast::<PyObject>();
    let derived_o = (&raw mut *derived).cast::<PyObject>();
    let unrelated_o = (&raw mut *unrelated).cast::<PyObject>();

    assert_eq!(
        unsafe { PyObject_IsSubclass(derived_o, base_o) },
        1,
        "derived IS a subclass of its base (mask-proof: exact-only returns 0)",
    );
    assert_eq!(
        unsafe { PyObject_IsSubclass(derived_o, derived_o) },
        1,
        "a class is a subclass of itself",
    );
    assert_eq!(
        unsafe { PyObject_IsSubclass(base_o, derived_o) },
        0,
        "the base is NOT a subclass of its derived (directional)",
    );
    assert_eq!(
        unsafe { PyObject_IsSubclass(derived_o, unrelated_o) },
        0,
        "unrelated classes are not in a subclass relationship",
    );
}

mod attribute_slot_readiness;

mod getset_member_descriptors;

mod native_descriptor_protocol;

mod object_attribute_bootstrap;

unsafe fn declare_bases(tp: &mut PyTypeObject, bases: &[*mut PyTypeObject]) {
    assert!(tp.tp_bases.is_null());
    unsafe {
        tp.tp_bases = molt_cpython_abi::api::sequences::PyTuple_New(bases.len() as Py_ssize_t);
        assert!(!tp.tp_bases.is_null());
        for (index, &base) in bases.iter().enumerate() {
            refcount::Py_INCREF(base.cast());
            assert_eq!(
                molt_cpython_abi::api::sequences::PyTuple_SetItem(
                    tp.tp_bases,
                    index as Py_ssize_t,
                    base.cast()
                ),
                0
            );
        }
    }
}

unsafe extern "C" fn right_repr(_object: *mut PyObject) -> *mut PyObject {
    unsafe { strings::PyUnicode_FromString(c"right".as_ptr()) }
}

#[test]
fn native_c3_metadata_and_slot_dispatch_agree_without_copying_declarations() {
    let _guard = init();
    let mut meta = NativeType::<PyTypeObject>::new();
    meta.tp_name = c"HierarchyMeta".as_ptr();
    meta.tp_base = &raw mut PyType_Type;
    assert_eq!(unsafe { ready(&mut *meta) }, 0);
    let mut root = NativeType::<PyTypeObject>::new();
    root.tp_name = c"HierarchyRoot".as_ptr();
    root.ob_base.ob_base.ob_type = &mut *meta;
    root.tp_repr = Some(slot_repr);
    assert_eq!(unsafe { ready(&mut *root) }, 0);
    let mut left = NativeType::<PyTypeObject>::new();
    left.tp_name = c"HierarchyLeft".as_ptr();
    left.tp_base = &mut *root;
    assert_eq!(unsafe { ready(&mut *left) }, 0);
    let mut right = NativeType::<PyTypeObject>::new();
    right.tp_name = c"HierarchyRight".as_ptr();
    right.tp_base = &mut *root;
    right.tp_repr = Some(right_repr);
    assert_eq!(unsafe { ready(&mut *right) }, 0);
    let mut child = NativeType::<PyTypeObject>::new();
    child.tp_name = c"HierarchyChild".as_ptr();
    child.tp_base = &mut *left;
    unsafe {
        declare_bases(&mut child, &[&mut *left, &mut *right]);
    }
    assert_eq!(unsafe { ready(&mut *child) }, 0);
    assert_eq!(child.ob_base.ob_base.ob_type, &raw mut *meta);
    assert!(child.tp_methods.is_null() && child.tp_members.is_null() && child.tp_getset.is_null());
    let expected = [
        &mut *child as *mut PyTypeObject,
        &mut *left,
        &mut *right,
        &mut *root,
        &raw mut PyBaseObject_Type,
    ];
    unsafe {
        use molt_cpython_abi::api::sequences::{PyTuple_GetItem, PyTuple_Size};
        assert_eq!(PyTuple_Size(child.tp_mro), 5);
        for (index, class) in expected.into_iter().enumerate() {
            assert_eq!(
                PyTuple_GetItem(child.tp_mro, index as Py_ssize_t),
                class.cast()
            );
        }
        let key = strings::PyUnicode_FromString(c"__repr__".as_ptr());
        assert!(mapping::PyDict_GetItemWithError(child.tp_dict, key).is_null());
        assert_eq!(
            typeobj::_PyType_Lookup(&mut *child, key),
            mapping::PyDict_GetItemWithError(right.tp_dict, key)
        );
        refcount::Py_DECREF(key);
        let mut instance = PyObject {
            ob_refcnt: 1,
            ob_type: &mut *child,
        };
        let rendered = typeobj::PyObject_Repr(&mut instance);
        assert!(!rendered.is_null());
        assert_eq!(
            std::ffi::CStr::from_ptr(strings::PyUnicode_AsUTF8(rendered)).to_bytes(),
            b"right"
        );
        refcount::Py_DECREF(rendered);
    }
    let mut duplicate = NativeType::<PyTypeObject>::new();
    duplicate.tp_name = c"DuplicateBases".as_ptr();
    duplicate.tp_base = &mut *left;
    unsafe {
        declare_bases(&mut duplicate, &[&mut *left, &mut *left]);
    }
    assert_eq!(unsafe { ready(&mut *duplicate) }, -1);
    assert_eq!(
        duplicate.tp_flags & (Py_TPFLAGS_READY | Py_TPFLAGS_READYING),
        0
    );
    assert!(duplicate.tp_mro.is_null());
    unsafe {
        assert_eq!(
            molt_cpython_abi::api::errors::PyErr_ExceptionMatches(
                (&raw mut PyExc_TypeError).cast()
            ),
            1
        );
        molt_cpython_abi::api::errors::PyErr_Clear();
    }
}

#[test]
fn compact_static_temporary_heap_flag_preserves_guard_and_inherited_tables() {
    use super::super::native_test_fixture::TypeStorage;
    use molt_cpython_abi::api::memory;

    // Reserve a sentinel span covering every possible heap-tail access so a
    // regression reports guard corruption without an out-of-allocation write.
    const GUARD_WORDS: usize = (std::mem::size_of::<PyHeapTypeObject>()
        - std::mem::size_of::<PyTypeObject>())
        / std::mem::size_of::<usize>()
        + 4;
    #[repr(C)]
    struct GuardedType {
        prefix: PyTypeObject,
        guard: [usize; GUARD_WORDS],
    }
    impl TypeStorage for GuardedType {
        unsafe fn allocate() -> *mut Self {
            unsafe { memory::PyObject_Calloc(1, std::mem::size_of::<Self>()).cast() }
        }
        unsafe fn free(pointer: *mut Self) {
            unsafe {
                memory::PyObject_Free(pointer.cast());
            }
        }
        fn type_object(&mut self) -> &mut PyTypeObject {
            &mut self.prefix
        }
    }
    unsafe extern "C" fn inherited_add(_: *mut PyObject, _: *mut PyObject) -> *mut PyObject {
        unsafe { molt_cpython_abi::api::object::Py_NewRef(&raw mut Py_None) }
    }

    let _guard = init();
    let mut number: PyNumberMethods = unsafe { std::mem::zeroed() };
    number.nb_add = inherited_add as *mut c_void;
    let mut base = NativeType::<PyTypeObject>::subtype(&raw mut PyBaseObject_Type, c"GuardedBase");
    base.tp_basicsize = std::mem::size_of::<PyObject>() as Py_ssize_t;
    base.tp_as_number = (&raw mut number).cast();
    assert_eq!(unsafe { base.ready() }, 0);
    let mut compact = NativeType::<GuardedType>::new();
    compact.guard = [0x1357_2468; GUARD_WORDS];
    let expected_guard = compact.guard;
    compact.prefix.ob_base.ob_base.ob_type = &raw mut PyType_Type;
    compact.prefix.tp_name = c"CythonTemporaryHeapFlag".as_ptr();
    compact.prefix.tp_base = &raw mut *base;
    compact.prefix.tp_flags = Py_TPFLAGS_DEFAULT | Py_TPFLAGS_HEAPTYPE;
    assert_ne!(
        unsafe { typeobj::molt_type_is_gc((&raw mut compact.prefix).cast()) },
        0
    );
    assert!(!unsafe {
        molt_cpython_abi::bridge::molt_foreign_object_is_gc_capable(
            (&raw mut compact.prefix).addr(),
        )
    });
    assert_eq!(unsafe { compact.ready() }, 0);
    assert_eq!(compact.guard, expected_guard);
    assert_eq!(compact.prefix.tp_as_number, base.tp_as_number);
    assert_eq!(
        unsafe {
            typeobj::PyType_GetSlot(
                &raw mut compact.prefix,
                molt_cpython_abi::type_slots::Py_nb_add,
            )
        },
        inherited_add as *mut c_void
    );
    unsafe {
        typeobj::PyType_Modified(&raw mut compact.prefix);
    }
    assert_eq!(compact.guard, expected_guard);
    compact.prefix.tp_flags &= !Py_TPFLAGS_HEAPTYPE;
    unsafe {
        typeobj::PyType_Modified(&raw mut compact.prefix);
    }
    assert_eq!(compact.guard, expected_guard);
}
