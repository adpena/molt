//! Type object API — PyType_Ready, PyType_GenericAlloc, Py_TYPE checks.

use crate::abi_types::{
    Py_TPFLAGS_HAVE_GC, Py_TPFLAGS_HEAPTYPE, Py_TPFLAGS_READY, Py_ssize_t, PyHeapTypeObject,
    PyObject, PyType_Spec, PyTypeObject,
};
use crate::bridge::GLOBAL_BRIDGE;
use molt_lang_obj_model::sequence_compare::RichCompareOp;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_long, c_longlong, c_ulong, c_ulonglong};
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

mod descriptors;
pub use descriptors::documentation_bytes;
mod hierarchy;
mod inheritance;
mod method_descriptors;
mod native_lifecycle;
mod native_slot_dispatch;
mod object_construction;
mod protocol_storage;
pub(crate) use object_construction::{
    object_init, object_new, object_repr, object_richcompare, object_str,
};
pub(crate) use protocol_storage::{TypeProtocolTables, process_runtime_protocols};
pub(crate) mod native_slot_mutation;
mod root_metadata;
pub use root_metadata::{TypeAttributeField, native_type_attribute_get, native_type_attribute_set};
mod slot_wrappers;
pub use descriptors::{PyDescr_NewGetSet, PyDescr_NewMember};
pub(crate) use method_descriptors::completes_call_operands as method_descriptor_completes_call_operands;
pub(crate) use method_descriptors::completes_vectorcall as method_descriptor_completes_vectorcall;
pub use method_descriptors::{
    PyClassMethod_New, PyDescr_NewClassMethod, PyDescr_NewMethod, PyStaticMethod_New,
};
pub(crate) use native_lifecycle::{
    NativeDeallocation, gc_uses_managed_storage, object_dealloc, type_dealloc,
};
pub(crate) use root_metadata::type_setattro;
use slot_wrappers::SLOT_WRAPPER_DEFS;
pub(crate) use slot_wrappers::completes_call_operands as slot_wrapper_completes_call_operands;
pub use slot_wrappers::{PyDescr_NewWrapper, PyWrapper_New};
pub(crate) use slot_wrappers::{setter_admitted, setter_type_admitted};

/// Build a real Init wrapper for a runtime integration witness. Runtime-bound
/// builtin namespaces contain managed semantic descriptors, so reading their
/// __init__ entries cannot provide a physical PyWrapperDescrObject. Reuse the
/// production slot declaration and public constructor without a throwaway type
/// or a second adapter implementation.
#[cfg(feature = "runtime-test-support")]
pub unsafe fn init_slot_wrapper_for_test(
    owner: *mut PyTypeObject,
    initializer: unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int,
) -> *mut PyObject {
    assert!(!owner.is_null());
    assert_ne!(unsafe { PyType_Check(owner.cast()) }, 0);
    let definition = SLOT_WRAPPER_DEFS
        .iter()
        .find(|definition| matches!(definition.slot, SlotWrapper::Direct(DirectSlot::Init)))
        .expect("native Init has a canonical slot declaration");
    assert_eq!(
        definition.base.offset as usize,
        std::mem::offset_of!(PyTypeObject, tp_init),
    );
    unsafe {
        PyDescr_NewWrapper(
            owner,
            (&raw const definition.base).cast_mut(),
            initializer as *const () as *mut c_void,
        )
    }
}

/// Exercise the production explicit attribute wrapper against managed Type
/// views. Their runtime namespaces are not physical PyWrapperDescrObjects.
#[cfg(feature = "runtime-test-support")]
pub unsafe fn attribute_slot_wrapper_for_test<const DELETE: bool>(
    owner: *mut PyTypeObject,
    setter: unsafe extern "C" fn(*mut PyObject, *mut PyObject, *mut PyObject) -> c_int,
) -> *mut PyObject {
    let name = if DELETE {
        c"__delattr__"
    } else {
        c"__setattr__"
    };
    let definition = SLOT_WRAPPER_DEFS
        .iter()
        .find(|definition| {
            matches!(definition.slot, SlotWrapper::Direct(DirectSlot::SetAttr))
                && unsafe { std::ffi::CStr::from_ptr(definition.base.name) } == name
        })
        .expect("native attribute mutation has canonical slot declarations");
    unsafe {
        PyDescr_NewWrapper(
            owner,
            (&raw const definition.base).cast_mut(),
            setter as *const () as *mut c_void,
        )
    }
}

static ABI_LOCAL_TYPES: Lazy<Mutex<HashMap<u32, usize>>> = Lazy::new(|| Mutex::new(HashMap::new()));
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TypeIdentity {
    address: usize,
    generation: u64,
}

impl TypeIdentity {
    fn is_live(self, registry: &TypeSubclassRegistry) -> bool {
        registry
            .live
            .get(&self.address)
            .is_some_and(|lifetime| lifetime.generation == self.generation)
    }

    /// The execution token protects header observation through retention.
    /// A registered type at zero references is still in its deallocator; it
    /// must not be resurrected by a non-owning subclass traversal.
    unsafe fn live_type(self, registry: &TypeSubclassRegistry) -> Option<*mut PyTypeObject> {
        if !self.is_live(registry) {
            return None;
        }
        let tp = ptr::with_exposed_provenance_mut::<PyTypeObject>(self.address);
        (unsafe { (*tp).ob_base.ob_base.ob_refcnt } != 0).then_some(tp)
    }
}

#[derive(Default)]
struct SubclassIdentities {
    order: Vec<TypeIdentity>,
    members: HashSet<TypeIdentity>,
}

impl SubclassIdentities {
    fn compact(&mut self) {
        self.order
            .retain(|identity| self.members.contains(identity));
        // Reclaim a retired cohort's high-water storage geometrically, keeping
        // registration and retirement amortized O(1) per edge.
        let retained = self.members.len().saturating_mul(2);
        if self.order.capacity() > retained.saturating_mul(2) {
            self.order.shrink_to(retained);
        }
        if self.members.capacity() > retained.saturating_mul(2) {
            self.members.shrink_to(retained);
        }
    }

    fn remove(&mut self, identity: &TypeIdentity) -> bool {
        self.members.remove(identity);
        if self.members.is_empty() {
            return true;
        }
        // Tombstones are bounded by the live cohort, independently of whether
        // any consumer ever traverses or modifies the base.
        if self.order.len() - self.members.len() > self.members.len() {
            self.compact();
        }
        false
    }
}

#[derive(Clone, Copy, Debug)]
enum TypeStorage {
    Unknown,
    NativeHeap { bytes: usize },
}

#[derive(Clone, Copy, Debug)]
struct TypeLifetime {
    generation: u64,
    storage: TypeStorage,
}

#[derive(Default)]
struct TypeSubclassRegistry {
    live: HashMap<usize, TypeLifetime>,
    subclasses: HashMap<TypeIdentity, SubclassIdentities>,
    bases_by_subclass: HashMap<TypeIdentity, HashSet<TypeIdentity>>,
    // Process-owned builtin shells remain closed across runtime teardown.
    // Only the next runtime bootstrap may reopen this exact cohort.
    retired_statics: HashSet<usize>,
}

static TYPE_SUBCLASSES: Lazy<Mutex<TypeSubclassRegistry>> =
    Lazy::new(|| Mutex::new(TypeSubclassRegistry::default()));
static NEXT_TYPE_IDENTITY_GENERATION: AtomicU64 = AtomicU64::new(1);
/// Insert the first observation of one allocation lifetime. Callers decide
/// whether they own a fresh allocation or are admitting more evidence for it.
fn insert_type_lifetime(
    registry: &mut TypeSubclassRegistry,
    address: usize,
    storage: TypeStorage,
) -> bool {
    debug_assert!(!registry.live.contains_key(&address));
    if registry.live.try_reserve(1).is_err() {
        return false;
    }
    let Ok(generation) =
        NEXT_TYPE_IDENTITY_GENERATION.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            value.checked_add(1)
        })
    else {
        return false;
    };
    registry.live.insert(
        address,
        TypeLifetime {
            generation,
            storage,
        },
    );
    true
}

/// The allocator owns fresh, unpublished storage. Any observation left at its
/// address belongs to an earlier allocation, including an Unknown receipt.
pub(crate) fn record_native_heap_type_allocation(address: usize, bytes: usize) -> bool {
    if bytes < std::mem::size_of::<PyHeapTypeObject>() {
        return false;
    }
    let mut registry = TYPE_SUBCLASSES.lock();
    retire_type_lifetime(&mut registry, address);
    insert_type_lifetime(&mut registry, address, TypeStorage::NativeHeap { bytes })
}

/// The selected tp_alloc contract promises the captured requested extent.
/// Generic allocation can provide a stronger actual-byte receipt; a custom
/// allocator is trusted exactly as CPython trusts its allocation contract.
/// No flag or post-callback metaclass value can manufacture this admission.
#[derive(Debug, PartialEq, Eq)]
enum TypeStorageAdmissionError {
    InsufficientExtent,
    Capacity,
}

fn admit_spec_type_allocation(
    address: usize,
    required: usize,
) -> Result<(), TypeStorageAdmissionError> {
    if required < std::mem::size_of::<PyHeapTypeObject>() {
        return Err(TypeStorageAdmissionError::InsufficientExtent);
    }
    let mut registry = TYPE_SUBCLASSES.lock();
    if let Some(lifetime) = registry.live.get_mut(&address) {
        // The allocator callback has already run. Readiness observations can
        // belong to this allocation; admitting its extent is not a new birth.
        return match lifetime.storage {
            TypeStorage::NativeHeap { bytes } if bytes < required => {
                Err(TypeStorageAdmissionError::InsufficientExtent)
            }
            TypeStorage::NativeHeap { .. } => Ok(()),
            TypeStorage::Unknown => {
                lifetime.storage = TypeStorage::NativeHeap { bytes: required };
                Ok(())
            }
        };
    }
    insert_type_lifetime(
        &mut registry,
        address,
        TypeStorage::NativeHeap { bytes: required },
    )
    .then_some(())
    .ok_or(TypeStorageAdmissionError::Capacity)
}

/// Extent permission, not a Python semantic flag. The caller owns a live type.
pub(crate) fn heap_type_storage(tp: *mut PyTypeObject) -> Option<*mut PyHeapTypeObject> {
    if tp.is_null() {
        return None;
    }
    if let Some(storage) = GLOBAL_BRIDGE.managed_type_storage(tp) {
        return storage;
    }
    if let Some(heap) = crate::abi_types::process_heap_type_storage(tp) {
        return Some(heap);
    }
    TYPE_SUBCLASSES
        .lock()
        .live
        .get(&tp.addr())
        .and_then(|lifetime| {
            matches!(lifetime.storage, TypeStorage::NativeHeap { .. }).then_some(tp.cast())
        })
}

type PyTypeWatchCallback = unsafe extern "C" fn(*mut PyObject) -> c_int;
const TYPE_MAX_WATCHERS: usize = 8;
const CANONICAL_INTERPRETER_ID: i64 = 0;
struct TypeWatcherState {
    interpreter_id: i64,
    callbacks: [Option<PyTypeWatchCallback>; TYPE_MAX_WATCHERS],
}

// Molt exposes one canonical interpreter (ID 0), with no subinterpreter
// creation surface. The mutex is the free-threaded watcher-state boundary.
static TYPE_WATCHER_STATE: Lazy<Mutex<TypeWatcherState>> = Lazy::new(|| {
    Mutex::new(TypeWatcherState {
        interpreter_id: CANONICAL_INTERPRETER_ID,
        callbacks: [None; TYPE_MAX_WATCHERS],
    })
});
static NEXT_TYPE_VERSION_TAG: AtomicU32 = AtomicU32::new(1);

fn type_identity(
    registry: &mut TypeSubclassRegistry,
    tp: *mut PyTypeObject,
) -> Option<TypeIdentity> {
    if tp.is_null() {
        return None;
    }
    let address = tp.addr();
    if registry.retired_statics.contains(&address) {
        return None;
    }
    let generation = if let Some(lifetime) = registry.live.get(&address) {
        lifetime.generation
    } else {
        if !insert_type_lifetime(registry, address, TypeStorage::Unknown) {
            return None;
        }
        registry.live[&address].generation
    };
    Some(TypeIdentity {
        address,
        generation,
    })
}

unsafe fn register_subclass(base: *mut PyTypeObject, subclass: *mut PyTypeObject) {
    if base.is_null() || subclass.is_null() || ptr::eq(base, subclass) {
        return;
    }
    let mut registry = TYPE_SUBCLASSES.lock();
    let Some(base_identity) = type_identity(&mut registry, base) else {
        return;
    };
    let Some(subclass_identity) = type_identity(&mut registry, subclass) else {
        return;
    };
    let subclasses = registry.subclasses.entry(base_identity).or_default();
    if subclasses.members.insert(subclass_identity) {
        subclasses.order.push(subclass_identity);
        registry
            .bases_by_subclass
            .entry(subclass_identity)
            .or_default()
            .insert(base_identity);
    }
}

/// Remove a non-owning type identity before its allocation is returned.
///
/// Every object-domain free routes here. Non-type addresses are absent and
/// cost one map probe; heap types lose both outgoing and incoming subclass
/// edges before address reuse can create a new generation.
pub(crate) fn unregister_type_address(address: usize) {
    if address == 0 {
        return;
    }
    retire_type_lifetime(&mut TYPE_SUBCLASSES.lock(), address);
}

/// One revocation primitive serves both terminal release and fresh allocation
/// recording, which must replace an old lifetime under the same registry lock.
fn retire_type_lifetime(registry: &mut TypeSubclassRegistry, address: usize) {
    let Some(lifetime) = registry.live.remove(&address) else {
        return;
    };
    let identity = TypeIdentity {
        address,
        generation: lifetime.generation,
    };
    if let Some(children) = registry.subclasses.remove(&identity) {
        for child in children.order {
            if let Some(bases) = registry.bases_by_subclass.get_mut(&child) {
                bases.remove(&identity);
                if bases.is_empty() {
                    registry.bases_by_subclass.remove(&child);
                }
            }
        }
    }
    if let Some(bases) = registry.bases_by_subclass.remove(&identity) {
        for base in bases {
            if registry
                .subclasses
                .get_mut(&base)
                .is_some_and(|children| children.remove(&identity))
            {
                registry.subclasses.remove(&base);
            }
        }
    }
}

/// Detach runtime-owned roots from an exact cohort of process-owned type shells.
///
/// This is not heap-type destruction or a foreign-type registry sweep. All
/// shells lose their live subclass identities before the first decref can
/// reenter the ABI. Runtime-derived readiness is invalidated; the boolean marks
/// an ordinary bootstrap shell whose process-ready state can survive if it has
/// no runtime-owned roots. The immutable C layout, base pointer, slot functions,
/// and immortal object headers survive for the next runtime.
///
/// # Safety
/// The caller must hold exclusive runtime teardown custody. Every non-null
/// pointer must name a live process-owned type shell, and each non-null dict,
/// bases, MRO, or cache field must own one reference in the still-live runtime.
pub(crate) unsafe fn retire_static_type_runtime_roots(types: &[(*mut PyTypeObject, bool)]) {
    {
        let mut registry = TYPE_SUBCLASSES.lock();
        registry.retired_statics.extend(
            types
                .iter()
                .filter(|(tp, _)| !tp.is_null())
                .map(|(tp, _)| tp.addr()),
        );
    }
    let mut seen = HashSet::with_capacity(types.len());
    let mut detached = Vec::with_capacity(types.len() * 4);
    for &(tp, bootstrap_ready) in types {
        if tp.is_null() || !seen.insert(tp.addr()) {
            continue;
        }
        unsafe {
            let first_root = detached.len();
            // Deduplicate shells, never owned edges: two fields may own two
            // references to the same object and must both be released.
            for field in [
                &raw mut (*tp).tp_dict,
                &raw mut (*tp).tp_bases,
                &raw mut (*tp).tp_mro,
                &raw mut (*tp).tp_cache,
            ] {
                let root = field.replace(ptr::null_mut());
                if !root.is_null() {
                    detached.push(root);
                }
            }
            if let Some(heap) = heap_type_storage(tp) {
                for field in [
                    &raw mut (*heap).ht_name,
                    &raw mut (*heap).ht_qualname,
                    &raw mut (*heap).ht_slots,
                    &raw mut (*heap).ht_module,
                ] {
                    let root = field.replace(ptr::null_mut());
                    if !root.is_null() {
                        detached.push(root);
                    }
                }
                // Runtime-renamed process shells own this byte allocation.
                let name = (&raw mut (*heap)._ht_tpname).replace(ptr::null_mut());
                if !name.is_null() {
                    (*tp).tp_name = c"<retired type>".as_ptr();
                    crate::api::memory::PyMem_Free(name.cast());
                }
            }
            let readiness = if bootstrap_ready && detached.len() == first_root {
                0
            } else {
                Py_TPFLAGS_READY
            };
            (*tp).tp_flags &= !(readiness
                | crate::abi_types::Py_TPFLAGS_READYING
                | crate::abi_types::Py_TPFLAGS_VALID_VERSION_TAG);
            (*tp).tp_version_tag = 0;
            (*tp).tp_watched = 0;
        }
        // No decrefs or callbacks occur under the subclass registry lock.
        // The process-wide generation and version counters remain monotonic.
        unregister_type_address(tp.addr());
    }
    for root in detached {
        unsafe { crate::api::refcount::Py_DECREF(root) };
    }
}

/// Reopen only the supplied process-owned shells at the next runtime bootstrap.
/// This does not resurrect old live identities, roots, version tags, or watches.
pub(crate) fn reopen_static_type_runtime_roots(types: &[(*mut PyTypeObject, bool)]) {
    let mut registry = TYPE_SUBCLASSES.lock();
    for &(tp, _) in types {
        registry.retired_statics.remove(&tp.addr());
    }
}

/// Callback addresses belong to the interpreter that registered them. Version
/// and allocation generations remain monotonic; a new runtime gets no old
/// extension callbacks, even if their slots were never explicitly cleared.
pub(crate) fn reset_type_watchers_for_runtime() {
    TYPE_WATCHER_STATE.lock().callbacks.fill(None);
}

struct TypeReadyingGuard(*mut PyTypeObject);

impl Drop for TypeReadyingGuard {
    fn drop(&mut self) {
        unsafe { (*self.0).tp_flags &= !crate::abi_types::Py_TPFLAGS_READYING };
    }
}

unsafe fn reject_type_readiness(message: &'static std::ffi::CStr) -> c_int {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_RuntimeError).cast(),
            message.as_ptr(),
        );
    }
    -1
}

unsafe fn register_type_subclasses(tp: *mut PyTypeObject) {
    let bases = unsafe { (*tp).tp_bases };
    if !bases.is_null() {
        let count = unsafe { crate::api::sequences::PyTuple_Size(bases) };
        if count >= 0 {
            for index in 0..count {
                let base = unsafe { crate::api::sequences::PyTuple_GetItem(bases, index) }
                    .cast::<PyTypeObject>();
                unsafe { register_subclass(base, tp) };
            }
            return;
        }
    }
    unsafe { register_subclass((*tp).tp_base, tp) };
}

unsafe fn reject_type_layout(message: &'static std::ffi::CStr) -> c_int {
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast::<PyObject>(),
            message.as_ptr(),
        )
    };
    -1
}

unsafe fn validate_base_layout(tp: *mut PyTypeObject, base: *mut PyTypeObject) -> c_int {
    if tp.is_null() || base.is_null() {
        return 0;
    }
    unsafe {
        if (*base).tp_flags & crate::abi_types::Py_TPFLAGS_BASETYPE == 0 {
            return reject_type_layout(c"type is not an acceptable base type");
        }
        if (*tp).tp_basicsize != 0 && (*tp).tp_basicsize < (*base).tp_basicsize {
            return reject_type_layout(c"type basicsize is smaller than its base layout");
        }
        if (*tp).tp_itemsize != 0
            && (*base).tp_itemsize != 0
            && (*tp).tp_itemsize != (*base).tp_itemsize
        {
            return reject_type_layout(c"type itemsize is incompatible with its base layout");
        }
    }
    0
}

/// Select one physical base using the shared solid-owner dominance rule.
unsafe fn acceptable_best_base(bases: *mut PyObject) -> *mut PyTypeObject {
    unsafe { hierarchy::best_base(bases) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_cpython_abi_type_canonicalize(
    kind: u32,
    type_obj: *mut PyTypeObject,
) -> *mut PyTypeObject {
    if kind == 0 || type_obj.is_null() {
        return ptr::null_mut();
    }

    let mut guard = ABI_LOCAL_TYPES.lock();
    if let Some(canonical) = guard.get(&kind) {
        return *canonical as *mut PyTypeObject;
    }

    let mut canonical: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
    unsafe {
        ptr::copy_nonoverlapping(type_obj, canonical.as_mut(), 1);
        if canonical.ob_base.ob_base.ob_type.is_null() {
            canonical.ob_base.ob_base.ob_type = &raw mut crate::abi_types::PyType_Type;
        }
    }
    let canonical = Box::into_raw(canonical);
    guard.insert(kind, canonical as usize);
    canonical
}

/// Mark a type as ready for use.
/// In Molt's bridge, static type objects are pre-initialized; heap types
/// need basic tp_base resolution.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Ready(tp: *mut PyTypeObject) -> c_int {
    unsafe { ready_type(&GLOBAL_BRIDGE, tp) }
}

/// The bridge that owns a projection also owns its readiness exposure. Native
/// callers enter through the process bridge; recursive publication supplies
/// its exact owner, including isolated bridge transactions.
pub(crate) unsafe fn ready_type(
    bridge: &crate::bridge::ObjectBridge,
    tp: *mut PyTypeObject,
) -> c_int {
    // Unconditional entry trace (before the null check) so *every* call site is
    // visible, including a null/unresolved `tp`. This distinguishes "the caller
    // was never reached" from "the caller passed a bad pointer": if a static
    // extension's exec sequence stops here we see the raw pointer that arrived.
    crate::capi_trace::trace_call("PyType_Ready:entry", Some(&format!("{:p}", tp)));
    if tp.is_null() {
        // A NULL type here is a real linkage/authority failure (a static
        // extension resolved a builtin type symbol such as `PyBool_Type` to a
        // null weak symbol). CPython would crash; we fail closed with an honest
        // record so the exec-failure path can name the exact site instead of
        // vanishing before any trace fires.
        crate::capi_trace::record_silent_failure("PyType_Ready", Some("null type"));
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let name = unsafe { (*tp).tp_name };
    if name.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                c"type has no resolved name".as_ptr(),
            )
        };
        return -1;
    }
    let label = if name.is_null() {
        format!("<unnamed@{:p}>", tp)
    } else {
        unsafe { std::ffi::CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    };
    crate::capi_trace::trace_call("PyType_Ready", Some(&label));

    // Retirement closes the entire builtin cohort before releasing any root.
    // A decref callback may use the old runtime, but cannot repopulate these
    // shells. Do not hold registry custody while reporting a C exception.
    let retired = TYPE_SUBCLASSES.lock().retired_statics.contains(&tp.addr());
    if retired {
        return unsafe { reject_type_readiness(c"builtin type belongs to a retired runtime") };
    }
    if unsafe { (*tp).tp_flags } & crate::abi_types::Py_TPFLAGS_READYING != 0 {
        return unsafe { reject_type_readiness(c"recursive PyType_Ready on an initializing type") };
    }

    if unsafe { (*tp).tp_flags } & Py_TPFLAGS_READY == 0
        && !bridge.managed_type_population_complete(tp)
    {
        return unsafe { reject_type_readiness(c"managed type metadata is still being published") };
    }
    let Some(_type_owner) =
        (unsafe { crate::api::refcount::OwnedPyObject::try_from_borrowed(tp.cast()) })
    else {
        return -1;
    };
    let binding = bridge.molt_handle_for_pyobj(tp.cast());

    // Bootstrap native shells carry initialized C slots before their runtime
    // roots exist. Projecting roots is not completion of native readiness.
    // Clear that bootstrap marker before fallible projection so a retry cannot
    // mistake partially populated roots for completed declaration admission.
    let completed = unsafe {
        (*tp).tp_flags & Py_TPFLAGS_READY != 0
            && !(*tp).tp_dict.is_null()
            && !(*tp).tp_bases.is_null()
            && !(*tp).tp_mro.is_null()
    };
    if !completed {
        unsafe {
            (*tp).tp_flags &= !Py_TPFLAGS_READY;
        }
    }

    // A managed projection has already staged its graph before entering this
    // pipeline. Guard even its exposure callbacks against recursive readiness.
    let early_readying = if !completed && bridge.type_uses_runtime_slots(tp) {
        unsafe {
            (*tp).tp_flags |= crate::abi_types::Py_TPFLAGS_READYING;
        }
        Some(TypeReadyingGuard(tp))
    } else {
        None
    };

    // Crossing a runtime-bound type through PyType_Ready exposes its real
    // namespace, including native method declarations, before any READY exit.
    let runtime_projection = match unsafe { bridge.expose_runtime_type_dictionary(tp) } {
        Ok(projection) => projection,
        Err(crate::ErrorIndicatorSet) => return -1,
    };
    if bridge.molt_handle_for_pyobj(tp.cast()) != binding {
        return unsafe { reject_type_readiness(c"type binding changed during namespace exposure") };
    }
    let managed = runtime_projection == Some(crate::bridge::RuntimeTypeProjection::RuntimeSlots);
    if runtime_projection.is_some()
        && (tp == &raw mut crate::abi_types::PyBaseObject_Type
            || tp == &raw mut crate::abi_types::PyType_Type)
    {
        // These bootstrap shells already own initialized physical slots. Their
        // mutually recursive runtime roots must not trigger native declarations.
        unsafe {
            register_type_subclasses(tp);
            install_metatype_getattro(tp);
            (*tp).tp_flags |= Py_TPFLAGS_READY;
        }
        return 0;
    }

    // Readiness owns C layout and subclass metadata, not a runtime reference.
    // Only an actual C-to-runtime crossing may acquire a foreign wrapper; in
    // particular, a runtime-bound builtin must never get a second identity.
    if completed {
        unsafe {
            register_type_subclasses(tp);
            install_metatype_getattro(tp);
        }
        return 0;
    }

    // READYING is the canonical recursion state, including allocation/error
    // callbacks during startup. Every success and failure path clears it.
    // Static shells initialize their C slots before a runtime dictionary exists.
    // READY is complete only once that namespace has been materialized. Retry a
    // failed materialization through the ordinary readiness transaction.
    unsafe {
        if (*tp).tp_flags & Py_TPFLAGS_HEAPTYPE == 0 {
            (*tp).tp_flags |= crate::abi_types::Py_TPFLAGS_IMMUTABLETYPE;
        }
        (*tp).tp_flags &= !Py_TPFLAGS_READY;
        (*tp).tp_flags |= crate::abi_types::Py_TPFLAGS_READYING;
    }
    let _readying = early_readying.unwrap_or_else(|| TypeReadyingGuard(tp));

    unsafe {
        if (*tp).tp_dict.is_null() {
            let dict = crate::api::mapping::PyDict_New();
            if dict.is_null() {
                return -1;
            }
            (*tp).tp_dict = dict;
        }
        // Metadata is complete before constructing any callback-bearing object.
        if hierarchy::prepare(bridge, tp, runtime_projection.is_some()) < 0 {
            return -1;
        }
        native_lifecycle::prepare_heap_defaults(tp, managed);
        inheritance::prepare_layout(tp);
        let declares_new = (*tp).tp_new.is_some()
            && (runtime_projection.is_none()
                || (*tp).tp_base.is_null()
                || (*tp).tp_new.map(|f| f as *const ())
                    != (*(*tp).tp_base).tp_new.map(|f| f as *const ()));
        inheritance::prepare_new(tp, managed);
        // Only this type's declarations introduce entries in its namespace.
        // METH_COEXIST can replace an operator wrapper; ordinary methods cannot.
        if !managed
            && ((declares_new && object_construction::add_new_wrapper(tp) < 0)
                || add_operators_to_dict(tp) < 0
                || add_methods_to_dict(tp) < 0
                || add_members_to_dict(tp) < 0
                || add_getset_to_dict(tp) < 0
                || root_metadata::add_type_documentation(tp) < 0)
        {
            return -1;
        }
        // Runtime classes keep their sealed graph and namespace. Physical
        // inheritance cannot synthesize __hash__ into that semantic authority.
        if inheritance::finish(tp, !managed) < 0
            || (managed && native_slot_mutation::initialize(bridge, tp) < 0)
        {
            return -1;
        }
        if bridge.molt_handle_for_pyobj(tp.cast()) != binding {
            return reject_type_readiness(c"type binding changed during readiness");
        }
        register_type_subclasses(tp);
        (*tp).tp_flags |= Py_TPFLAGS_READY;
    }
    drop(_readying);

    unsafe {
        install_metatype_getattro(tp);
    }
    0
}

/// Publish native declarations as descriptors. Names already present in the
/// namespace win unless the declaration explicitly requests METH_COEXIST.
unsafe fn add_methods_to_dict(tp: *mut PyTypeObject) -> c_int {
    use crate::abi_types::{METH_CLASS, METH_COEXIST, METH_STATIC};
    use crate::api::refcount::OwnedPyObject;
    unsafe {
        let mut method = (*tp).tp_methods;
        if method.is_null() {
            return 0;
        }
        while !(*method).ml_name.is_null() {
            let flags = (*method).ml_flags;
            if flags & (METH_CLASS | METH_STATIC) == (METH_CLASS | METH_STATIC) {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_ValueError).cast(),
                    c"method cannot be both class and static".as_ptr(),
                );
                return -1;
            }
            let descriptor = OwnedPyObject::from_owned(if flags & METH_CLASS != 0 {
                PyDescr_NewClassMethod(tp, method)
            } else if flags & METH_STATIC != 0 {
                let callable = OwnedPyObject::from_owned(crate::api::object::PyCMethod_New(
                    method,
                    tp.cast(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                ));
                if callable.as_ptr().is_null() {
                    return -1;
                }
                PyStaticMethod_New(callable.as_ptr())
            } else {
                PyDescr_NewMethod(tp, method)
            });
            if descriptor.as_ptr().is_null() {
                return -1;
            }
            let name = OwnedPyObject::from_owned(crate::api::strings::PyUnicode_FromString(
                (*method).ml_name,
            ));
            if name.as_ptr().is_null() {
                return -1;
            }
            let status = if flags & METH_COEXIST != 0 {
                crate::api::mapping::PyDict_SetItem(
                    (*tp).tp_dict,
                    name.as_ptr(),
                    descriptor.as_ptr(),
                )
            } else if crate::api::mapping::PyDict_SetDefault(
                (*tp).tp_dict,
                name.as_ptr(),
                descriptor.as_ptr(),
            )
            .is_null()
            {
                -1
            } else {
                0
            };
            if status < 0 {
                return -1;
            }
            method = method.add(1);
        }
        0
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SlotWrapper {
    Direct(DirectSlot),
    Number(NumberSlot),
    Sequence(SequenceSlot),
    Mapping(MappingSlot),
    Async(AsyncSlot),
    Buffer(BufferSlot),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DirectSlot {
    Alloc,
    Base,
    Bases,
    Repr,
    Hash,
    Call,
    Clear,
    Dealloc,
    Del,
    Str,
    Doc,
    LegacyGetAttr,
    GetAttr,
    LegacySetAttr,
    SetAttr,
    RichCompare,
    IsGc,
    Iter,
    IterNext,
    Methods,
    New,
    DescrGet,
    DescrSet,
    Init,
    Traverse,
    Members,
    GetSet,
    Free,
    Finalize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NumberSlot {
    Divmod,
    Add,
    Subtract,
    Multiply,
    Remainder,
    Power,
    Negative,
    Positive,
    Absolute,
    Bool,
    Invert,
    LShift,
    RShift,
    And,
    Xor,
    Or,
    Int,
    Float,
    InPlaceAdd,
    InPlaceSubtract,
    InPlaceMultiply,
    InPlaceRemainder,
    InPlacePower,
    InPlaceLShift,
    InPlaceRShift,
    InPlaceAnd,
    InPlaceXor,
    InPlaceOr,
    FloorDivide,
    TrueDivide,
    InPlaceFloorDivide,
    InPlaceTrueDivide,
    Index,
    MatrixMultiply,
    InPlaceMatrixMultiply,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SequenceSlot {
    Length,
    Concat,
    Repeat,
    Item,
    AssItem,
    Contains,
    InPlaceConcat,
    InPlaceRepeat,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MappingSlot {
    Length,
    Subscript,
    AssSubscript,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AsyncSlot {
    Await,
    Iter,
    Next,
    Send,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BufferSlot {
    Get,
    Release,
}

/// Return the raw pointer-sized storage that owns one direct type slot. Rust's
/// FFI `Option<extern "C" fn>` fields use the nullable-pointer representation;
/// the compile-time size/alignment assertions below make that assumption
/// explicit. Both FromSpec writes and GetSlot reads go through this address, so
/// the stable slot map cannot drift into independent setter/getter authorities.
unsafe fn direct_slot_storage(tp: *mut PyTypeObject, slot: DirectSlot) -> *mut *mut c_void {
    const _: () = assert!(
        std::mem::size_of::<Option<unsafe extern "C" fn(*mut PyObject)>>()
            == std::mem::size_of::<*mut c_void>()
    );
    const _: () = assert!(
        std::mem::align_of::<Option<unsafe extern "C" fn(*mut PyObject)>>()
            == std::mem::align_of::<*mut c_void>()
    );
    macro_rules! storage {
        ($field:ident) => {
            std::ptr::addr_of_mut!((*tp).$field).cast::<*mut c_void>()
        };
    }
    unsafe {
        match slot {
            DirectSlot::Alloc => storage!(tp_alloc),
            DirectSlot::Base => storage!(tp_base),
            DirectSlot::Bases => storage!(tp_bases),
            DirectSlot::Repr => storage!(tp_repr),
            DirectSlot::Hash => storage!(tp_hash),
            DirectSlot::Call => storage!(tp_call),
            DirectSlot::Clear => storage!(tp_clear),
            DirectSlot::Dealloc => storage!(tp_dealloc),
            DirectSlot::Del => storage!(tp_del),
            DirectSlot::Str => storage!(tp_str),
            DirectSlot::Doc => storage!(tp_doc),
            DirectSlot::LegacyGetAttr => storage!(tp_getattr),
            DirectSlot::GetAttr => storage!(tp_getattro),
            DirectSlot::LegacySetAttr => storage!(tp_setattr),
            DirectSlot::SetAttr => storage!(tp_setattro),
            DirectSlot::RichCompare => storage!(tp_richcompare),
            DirectSlot::IsGc => storage!(tp_is_gc),
            DirectSlot::Iter => storage!(tp_iter),
            DirectSlot::IterNext => storage!(tp_iternext),
            DirectSlot::Methods => storage!(tp_methods),
            DirectSlot::New => storage!(tp_new),
            DirectSlot::DescrGet => storage!(tp_descr_get),
            DirectSlot::DescrSet => storage!(tp_descr_set),
            DirectSlot::Init => storage!(tp_init),
            DirectSlot::Traverse => storage!(tp_traverse),
            DirectSlot::Members => storage!(tp_members),
            DirectSlot::GetSet => storage!(tp_getset),
            DirectSlot::Free => storage!(tp_free),
            DirectSlot::Finalize => storage!(tp_finalize),
        }
    }
}

/// Return the one pointer-sized storage cell for a public Stable-ABI slot.
/// Protocol tables are created only for FromSpec writes; GetSlot reads a missing
/// parent as a valid NULL slot without allocating or setting an exception.
unsafe fn slot_wrapper_storage(tp: *mut PyTypeObject, slot: SlotWrapper) -> *mut *mut c_void {
    macro_rules! field_storage {
        ($table:expr, $field:ident) => {
            std::ptr::addr_of_mut!((*$table).$field)
        };
    }
    unsafe {
        match slot {
            SlotWrapper::Direct(slot) => direct_slot_storage(tp, slot),
            SlotWrapper::Number(slot) => {
                let table = (*tp)
                    .tp_as_number
                    .cast::<crate::abi_types::PyNumberMethods>();
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    NumberSlot::Divmod => field_storage!(table, nb_divmod),
                    NumberSlot::Add => field_storage!(table, nb_add),
                    NumberSlot::Subtract => field_storage!(table, nb_subtract),
                    NumberSlot::Multiply => field_storage!(table, nb_multiply),
                    NumberSlot::Remainder => field_storage!(table, nb_remainder),
                    NumberSlot::Power => field_storage!(table, nb_power),
                    NumberSlot::Negative => field_storage!(table, nb_negative),
                    NumberSlot::Positive => field_storage!(table, nb_positive),
                    NumberSlot::Absolute => field_storage!(table, nb_absolute),
                    NumberSlot::Bool => field_storage!(table, nb_bool),
                    NumberSlot::Invert => field_storage!(table, nb_invert),
                    NumberSlot::LShift => field_storage!(table, nb_lshift),
                    NumberSlot::RShift => field_storage!(table, nb_rshift),
                    NumberSlot::And => field_storage!(table, nb_and),
                    NumberSlot::Xor => field_storage!(table, nb_xor),
                    NumberSlot::Or => field_storage!(table, nb_or),
                    NumberSlot::Int => field_storage!(table, nb_int),
                    NumberSlot::Float => field_storage!(table, nb_float),
                    NumberSlot::InPlaceAdd => field_storage!(table, nb_inplace_add),
                    NumberSlot::InPlaceSubtract => {
                        field_storage!(table, nb_inplace_subtract)
                    }
                    NumberSlot::InPlaceMultiply => {
                        field_storage!(table, nb_inplace_multiply)
                    }
                    NumberSlot::InPlaceRemainder => {
                        field_storage!(table, nb_inplace_remainder)
                    }
                    NumberSlot::InPlacePower => field_storage!(table, nb_inplace_power),
                    NumberSlot::InPlaceLShift => field_storage!(table, nb_inplace_lshift),
                    NumberSlot::InPlaceRShift => field_storage!(table, nb_inplace_rshift),
                    NumberSlot::InPlaceAnd => field_storage!(table, nb_inplace_and),
                    NumberSlot::InPlaceXor => field_storage!(table, nb_inplace_xor),
                    NumberSlot::InPlaceOr => field_storage!(table, nb_inplace_or),
                    NumberSlot::FloorDivide => field_storage!(table, nb_floor_divide),
                    NumberSlot::TrueDivide => field_storage!(table, nb_true_divide),
                    NumberSlot::InPlaceFloorDivide => {
                        field_storage!(table, nb_inplace_floor_divide)
                    }
                    NumberSlot::InPlaceTrueDivide => {
                        field_storage!(table, nb_inplace_true_divide)
                    }
                    NumberSlot::Index => field_storage!(table, nb_index),
                    NumberSlot::MatrixMultiply => field_storage!(table, nb_matrix_multiply),
                    NumberSlot::InPlaceMatrixMultiply => {
                        field_storage!(table, nb_inplace_matrix_multiply)
                    }
                }
            }
            SlotWrapper::Sequence(slot) => {
                let table = (*tp)
                    .tp_as_sequence
                    .cast::<crate::abi_types::PySequenceMethods>();
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    SequenceSlot::Length => field_storage!(table, sq_length),
                    SequenceSlot::Concat => field_storage!(table, sq_concat),
                    SequenceSlot::Repeat => field_storage!(table, sq_repeat),
                    SequenceSlot::Item => field_storage!(table, sq_item),
                    SequenceSlot::AssItem => field_storage!(table, sq_ass_item),
                    SequenceSlot::Contains => field_storage!(table, sq_contains),
                    SequenceSlot::InPlaceConcat => field_storage!(table, sq_inplace_concat),
                    SequenceSlot::InPlaceRepeat => field_storage!(table, sq_inplace_repeat),
                }
            }
            SlotWrapper::Mapping(slot) => {
                let table = (*tp)
                    .tp_as_mapping
                    .cast::<crate::abi_types::PyMappingMethods>();
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    MappingSlot::Length => field_storage!(table, mp_length),
                    MappingSlot::Subscript => field_storage!(table, mp_subscript),
                    MappingSlot::AssSubscript => field_storage!(table, mp_ass_subscript),
                }
            }
            SlotWrapper::Async(slot) => {
                let table = (*tp).tp_as_async.cast::<crate::abi_types::PyAsyncMethods>();
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    AsyncSlot::Await => field_storage!(table, am_await),
                    AsyncSlot::Iter => field_storage!(table, am_aiter),
                    AsyncSlot::Next => field_storage!(table, am_anext),
                    AsyncSlot::Send => field_storage!(table, am_send),
                }
            }
            SlotWrapper::Buffer(slot) => {
                let table = (*tp).tp_as_buffer.cast::<crate::abi_types::PyBufferProcs>();
                if table.is_null() {
                    return ptr::null_mut();
                }
                match slot {
                    BufferSlot::Get => field_storage!(table, bf_getbuffer),
                    BufferSlot::Release => field_storage!(table, bf_releasebuffer),
                }
            }
        }
    }
}

unsafe fn slot_wrapper_ptr(tp: *mut PyTypeObject, slot: SlotWrapper) -> *mut c_void {
    let storage = unsafe { slot_wrapper_storage(tp, slot) };
    if storage.is_null() {
        ptr::null_mut()
    } else {
        unsafe { storage.read() }
    }
}

pub(crate) type BfGetBuffer =
    unsafe extern "C" fn(*mut PyObject, *mut crate::abi_types::Py_buffer, c_int) -> c_int;
pub(crate) type BfReleaseBuffer =
    unsafe extern "C" fn(*mut PyObject, *mut crate::abi_types::Py_buffer);

/// Buffer callbacks share the physical slot authority used by PyType_GetSlot.
/// Callers pin the object/type; no table borrow crosses a callback.
pub(crate) unsafe fn type_bf_getbuffer(tp: *mut PyTypeObject) -> Option<BfGetBuffer> {
    if tp.is_null() {
        return None;
    }
    let raw = unsafe { slot_wrapper_ptr(tp, SlotWrapper::Buffer(BufferSlot::Get)) };
    if raw.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute::<*mut c_void, BfGetBuffer>(raw) })
    }
}

pub(crate) unsafe fn type_bf_releasebuffer(tp: *mut PyTypeObject) -> Option<BfReleaseBuffer> {
    if tp.is_null() {
        return None;
    }
    let raw = unsafe { slot_wrapper_ptr(tp, SlotWrapper::Buffer(BufferSlot::Release)) };
    if raw.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute::<*mut c_void, BfReleaseBuffer>(raw) })
    }
}

fn stable_slot_wrapper(slot: c_int) -> Option<SlotWrapper> {
    use AsyncSlot as A;
    use BufferSlot as B;
    use DirectSlot as D;
    use MappingSlot as M;
    use NumberSlot as N;
    use SequenceSlot as S;
    Some(match slot {
        ts::Py_bf_getbuffer => SlotWrapper::Buffer(B::Get),
        ts::Py_bf_releasebuffer => SlotWrapper::Buffer(B::Release),
        ts::Py_mp_ass_subscript => SlotWrapper::Mapping(M::AssSubscript),
        ts::Py_mp_length => SlotWrapper::Mapping(M::Length),
        ts::Py_mp_subscript => SlotWrapper::Mapping(M::Subscript),
        ts::Py_nb_absolute => SlotWrapper::Number(N::Absolute),
        ts::Py_nb_add => SlotWrapper::Number(N::Add),
        ts::Py_nb_and => SlotWrapper::Number(N::And),
        ts::Py_nb_bool => SlotWrapper::Number(N::Bool),
        ts::Py_nb_divmod => SlotWrapper::Number(N::Divmod),
        ts::Py_nb_float => SlotWrapper::Number(N::Float),
        ts::Py_nb_floor_divide => SlotWrapper::Number(N::FloorDivide),
        ts::Py_nb_index => SlotWrapper::Number(N::Index),
        ts::Py_nb_inplace_add => SlotWrapper::Number(N::InPlaceAdd),
        ts::Py_nb_inplace_and => SlotWrapper::Number(N::InPlaceAnd),
        ts::Py_nb_inplace_floor_divide => SlotWrapper::Number(N::InPlaceFloorDivide),
        ts::Py_nb_inplace_lshift => SlotWrapper::Number(N::InPlaceLShift),
        ts::Py_nb_inplace_multiply => SlotWrapper::Number(N::InPlaceMultiply),
        ts::Py_nb_inplace_or => SlotWrapper::Number(N::InPlaceOr),
        ts::Py_nb_inplace_power => SlotWrapper::Number(N::InPlacePower),
        ts::Py_nb_inplace_remainder => SlotWrapper::Number(N::InPlaceRemainder),
        ts::Py_nb_inplace_rshift => SlotWrapper::Number(N::InPlaceRShift),
        ts::Py_nb_inplace_subtract => SlotWrapper::Number(N::InPlaceSubtract),
        ts::Py_nb_inplace_true_divide => SlotWrapper::Number(N::InPlaceTrueDivide),
        ts::Py_nb_inplace_xor => SlotWrapper::Number(N::InPlaceXor),
        ts::Py_nb_int => SlotWrapper::Number(N::Int),
        ts::Py_nb_invert => SlotWrapper::Number(N::Invert),
        ts::Py_nb_lshift => SlotWrapper::Number(N::LShift),
        ts::Py_nb_multiply => SlotWrapper::Number(N::Multiply),
        ts::Py_nb_negative => SlotWrapper::Number(N::Negative),
        ts::Py_nb_or => SlotWrapper::Number(N::Or),
        ts::Py_nb_positive => SlotWrapper::Number(N::Positive),
        ts::Py_nb_power => SlotWrapper::Number(N::Power),
        ts::Py_nb_remainder => SlotWrapper::Number(N::Remainder),
        ts::Py_nb_rshift => SlotWrapper::Number(N::RShift),
        ts::Py_nb_subtract => SlotWrapper::Number(N::Subtract),
        ts::Py_nb_true_divide => SlotWrapper::Number(N::TrueDivide),
        ts::Py_nb_xor => SlotWrapper::Number(N::Xor),
        ts::Py_sq_ass_item => SlotWrapper::Sequence(S::AssItem),
        ts::Py_sq_concat => SlotWrapper::Sequence(S::Concat),
        ts::Py_sq_contains => SlotWrapper::Sequence(S::Contains),
        ts::Py_sq_inplace_concat => SlotWrapper::Sequence(S::InPlaceConcat),
        ts::Py_sq_inplace_repeat => SlotWrapper::Sequence(S::InPlaceRepeat),
        ts::Py_sq_item => SlotWrapper::Sequence(S::Item),
        ts::Py_sq_length => SlotWrapper::Sequence(S::Length),
        ts::Py_sq_repeat => SlotWrapper::Sequence(S::Repeat),
        ts::Py_tp_alloc => SlotWrapper::Direct(D::Alloc),
        ts::Py_tp_base => SlotWrapper::Direct(D::Base),
        ts::Py_tp_bases => SlotWrapper::Direct(D::Bases),
        ts::Py_tp_call => SlotWrapper::Direct(D::Call),
        ts::Py_tp_clear => SlotWrapper::Direct(D::Clear),
        ts::Py_tp_dealloc => SlotWrapper::Direct(D::Dealloc),
        ts::Py_tp_del => SlotWrapper::Direct(D::Del),
        ts::Py_tp_descr_get => SlotWrapper::Direct(D::DescrGet),
        ts::Py_tp_descr_set => SlotWrapper::Direct(D::DescrSet),
        ts::Py_tp_doc => SlotWrapper::Direct(D::Doc),
        ts::Py_tp_getattr => SlotWrapper::Direct(D::LegacyGetAttr),
        ts::Py_tp_getattro => SlotWrapper::Direct(D::GetAttr),
        ts::Py_tp_hash => SlotWrapper::Direct(D::Hash),
        ts::Py_tp_init => SlotWrapper::Direct(D::Init),
        ts::Py_tp_is_gc => SlotWrapper::Direct(D::IsGc),
        ts::Py_tp_iter => SlotWrapper::Direct(D::Iter),
        ts::Py_tp_iternext => SlotWrapper::Direct(D::IterNext),
        ts::Py_tp_methods => SlotWrapper::Direct(D::Methods),
        ts::Py_tp_new => SlotWrapper::Direct(D::New),
        ts::Py_tp_repr => SlotWrapper::Direct(D::Repr),
        ts::Py_tp_richcompare => SlotWrapper::Direct(D::RichCompare),
        ts::Py_tp_setattr => SlotWrapper::Direct(D::LegacySetAttr),
        ts::Py_tp_setattro => SlotWrapper::Direct(D::SetAttr),
        ts::Py_tp_str => SlotWrapper::Direct(D::Str),
        ts::Py_tp_traverse => SlotWrapper::Direct(D::Traverse),
        ts::Py_tp_members => SlotWrapper::Direct(D::Members),
        ts::Py_tp_getset => SlotWrapper::Direct(D::GetSet),
        ts::Py_tp_free => SlotWrapper::Direct(D::Free),
        ts::Py_nb_matrix_multiply => SlotWrapper::Number(N::MatrixMultiply),
        ts::Py_nb_inplace_matrix_multiply => SlotWrapper::Number(N::InPlaceMatrixMultiply),
        ts::Py_am_await => SlotWrapper::Async(A::Await),
        ts::Py_am_aiter => SlotWrapper::Async(A::Iter),
        ts::Py_am_anext => SlotWrapper::Async(A::Next),
        ts::Py_tp_finalize => SlotWrapper::Direct(D::Finalize),
        ts::Py_am_send => SlotWrapper::Async(A::Send),
        _ => return None,
    })
}

/// Return one CPython Stable-ABI type-slot value for every public id 1..=81.
/// The numeric ids come from the single generated authority; the lookup reads
/// the same concrete fields that `PyType_FromSpec*` populates.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetSlot(tp: *mut PyTypeObject, slot: c_int) -> *mut c_void {
    if tp.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let Some(wrapper) = stable_slot_wrapper(slot) else {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    };
    unsafe { slot_wrapper_ptr(tp, wrapper) }
}

unsafe fn add_operators_to_dict(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        let dict = (*tp).tp_dict;
        for def in SLOT_WRAPPER_DEFS {
            // Mutation-only declarations (__getattr__/__new__) have their own
            // Python publication protocols and no physical wrapper adapter.
            if def.base.wrapper.is_none() {
                continue;
            }
            // Builtin exception rendering declarations come from the shared
            // schema. PyType_Ready has already inherited C slots, but an
            // inherited slot does not introduce a descriptor in this dict.
            if let Some(spec) = crate::abi_types::exc_singleton_name(tp.cast())
                .and_then(|name| name.strip_prefix("PyExc_"))
                .and_then(molt_lang_obj_model::builtin_exception_spec)
                && ((matches!(def.slot, SlotWrapper::Direct(DirectSlot::Repr))
                    && !spec.declares_repr())
                    || (matches!(def.slot, SlotWrapper::Direct(DirectSlot::Str))
                        && spec.declared_str_slot().is_none()))
            {
                continue;
            }
            let wrapped = slot_wrapper_ptr(tp, def.slot);
            if wrapped.is_null() || wrapped == def.base.function {
                continue;
            }
            let name = def.base.name;
            let existing = crate::api::mapping::_PyDict_GetItemStringWithError(dict, name);
            if descriptors::pending() {
                return -1;
            }
            if !existing.is_null() {
                continue;
            }
            let hash_not_implemented = matches!(def.slot, SlotWrapper::Direct(DirectSlot::Hash))
                && wrapped == PyObject_HashNotImplemented as *const () as *mut c_void;
            if hash_not_implemented {
                if crate::api::mapping::PyDict_SetItemString(
                    dict,
                    name,
                    &raw mut crate::abi_types::Py_None,
                ) < 0
                {
                    return -1;
                }
            } else {
                let descr = PyDescr_NewWrapper(tp, (&raw const def.base).cast_mut(), wrapped);
                if descr.is_null() {
                    return -1;
                }
                let stored = crate::api::mapping::PyDict_SetItemString(dict, name, descr);
                crate::api::errors::release_preserving_error(&[descr]);
                if stored < 0 {
                    return -1;
                }
            }
        }
        0
    }
}

/// Publish one owned descriptor with normal dictionary ownership, then consume
/// the constructor reference. The runtime dictionary/foreign bridge owns the
/// stored edge; publication must not leave a second permanent C anchor.
unsafe fn store_descr(dict: *mut PyObject, descr: *mut PyObject) -> c_int {
    let name = unsafe { PyDescr_NAME(descr) };
    let status = if dict.is_null() || name.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        -1
    } else {
        let stored = unsafe { crate::api::mapping::PyDict_SetDefault(dict, name, descr) };
        unsafe {
            crate::api::errors::check_native_status(
                if stored.is_null() { -1 } else { 0 },
                "native descriptor publication",
            )
        }
    };
    unsafe { crate::api::errors::release_preserving_error(&[descr]) };
    status
}

/// Populate `tp`'s `tp_dict` with a `member_descriptor` for each entry in its
/// own `tp_members` table. Mirrors CPython's `type_add_members`.
unsafe fn add_members_to_dict(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        let mut memb = (*tp).tp_members;
        if memb.is_null() {
            return 0;
        }
        let dict = (*tp).tp_dict;
        while !(*memb).name.is_null() {
            let descr = PyDescr_NewMember(tp, memb);
            if descr.is_null() {
                // PyDescr_NewMember recorded a silent failure; set an honest
                // exception if the alloc layer left none pending.
                if crate::api::errors::PyErr_Occurred().is_null() {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"PyDescr_NewMember returned NULL during PyType_Ready".as_ptr(),
                    );
                }
                return -1;
            }
            if store_descr(dict, descr) < 0 {
                return -1;
            }
            memb = memb.add(1);
        }
        0
    }
}

/// Populate `tp`'s `tp_dict` with a `getset_descriptor` for each entry in its
/// own `tp_getset` table. Mirrors CPython's `type_add_getset`.
unsafe fn add_getset_to_dict(tp: *mut PyTypeObject) -> c_int {
    unsafe {
        let mut gsp = (*tp).tp_getset;
        if gsp.is_null() {
            return 0;
        }
        let dict = (*tp).tp_dict;
        while !(*gsp).name.is_null() {
            let descr = PyDescr_NewGetSet(tp, gsp);
            if descr.is_null() {
                if crate::api::errors::PyErr_Occurred().is_null() {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"PyDescr_NewGetSet returned NULL during PyType_Ready".as_ptr(),
                    );
                }
                return -1;
            }
            if store_descr(dict, descr) < 0 {
                return -1;
            }
            gsp = gsp.add(1);
        }
        0
    }
}

/// CPython 3.12 `type_is_gc` — the `tp_is_gc` slot of `PyType_Type`.
///
/// `type` itself advertises `Py_TPFLAGS_HAVE_GC`, but only heap-allocated type
/// objects may participate in cycles and be traversed by the collector.  This
/// predicate is therefore deliberately keyed on the candidate type object's
/// `Py_TPFLAGS_HEAPTYPE` bit, not on `Py_TPFLAGS_HAVE_GC`.  C extensions call
/// this slot directly (numpy's `_DTypeMeta` does so while deciding whether a
/// dtype metaclass instance is GC-tracked), so the canonical `PyType_Type`
/// must publish a real callable rather than relying on the bridge's optional-
/// slot fallback.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_type_is_gc(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    let ty = op.cast::<PyTypeObject>();
    (unsafe { (*ty).tp_flags } & Py_TPFLAGS_HEAPTYPE) as c_int
}

type TypeVisitProc = unsafe extern "C" fn(*mut PyObject, *mut c_void) -> c_int;

/// CPython 3.12 ``type_traverse`` for heap type objects.
///
/// ``type_is_gc`` prevents the collector from invoking this slot for static
/// types. Heap types own references through the common type header plus
/// ``PyHeapTypeObject.ht_module``; visiting this exact family is what makes
/// class/dict/MRO/module cycles observable to the collector.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_type_traverse(
    op: *mut PyObject,
    visit_raw: *mut c_void,
    arg: *mut c_void,
) -> c_int {
    if op.is_null() || visit_raw.is_null() {
        return 0;
    }
    if crate::bridge::GLOBAL_BRIDGE
        .managed_handle_for_pyobj(op)
        .is_some()
    {
        return unsafe { crate::api::memory::molt_managed_gc_traverse(op, visit_raw, arg) };
    }
    let type_ = op.cast::<PyTypeObject>();
    let Some(heap) = heap_type_storage(type_) else {
        return 0;
    };
    let visit: TypeVisitProc = unsafe { std::mem::transmute(visit_raw) };
    let references = unsafe {
        [
            (*type_).tp_dict,
            (*type_).tp_cache,
            (*type_).tp_mro,
            (*type_).tp_bases,
            (*type_).tp_base.cast::<PyObject>(),
            (*heap).ht_name,
            (*heap).ht_qualname,
            (*heap).ht_slots,
            (*heap).ht_module,
        ]
    };
    for reference in references {
        if reference.is_null() {
            continue;
        }
        let rc = unsafe { visit(reference, arg) };
        if rc != 0 {
            return rc;
        }
    }
    0
}

/// CPython 3.12 ``type_clear`` for heap type objects.
///
/// Invalidate method-cache authority before clearing the type dict, then break
/// the two hard ownership cycles CPython clears here: ``ht_module`` and
/// ``tp_mro``. Bases/cache/subclasses/slot-name tuples are deliberately retained
/// for the same ownership reasons as CPython's implementation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_type_clear(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    if crate::bridge::GLOBAL_BRIDGE
        .managed_handle_for_pyobj(op)
        .is_some()
    {
        return unsafe { crate::api::memory::molt_managed_gc_clear(op) };
    }
    let type_ = op.cast::<PyTypeObject>();
    let Some(heap) = heap_type_storage(type_) else {
        return 0;
    };
    unsafe {
        PyType_Modified(type_);
        if !(*type_).tp_dict.is_null() {
            crate::api::mapping::PyDict_Clear((*type_).tp_dict);
        }
        crate::api::refcount::Py_CLEAR(&raw mut (*heap).ht_module);
        crate::api::refcount::Py_CLEAR(&raw mut (*type_).tp_mro);
    }
    0
}

/// CPython 3.12 `type_call` — the `tp_call` slot of `PyType_Type`. Verified
/// verbatim against the primary source (python/cpython v3.12.13
/// `Objects/typeobject.c::type_call`): the `type(x)` one-argument special case
/// (only for `type` itself, #27157), the "type() takes 1 or 3 arguments"
/// error, the NULL-`tp_new` "cannot create '%s' instances" error, the
/// `tp_new` → `PyObject_TypeCheck` → `tp_init` flow with `res < 0` dropping
/// the fresh instance, and `_Py_CheckFunctionResult`'s fail-closed contract
/// (NULL without an exception ⇒ SystemError; a result with an exception
/// pending ⇒ SystemError) instead of CPython's debug-only asserts.
///
/// Installed on `PyType_Type` by the process ABI bootstrap, so every C-extension
/// metatype that sets `tp_base = &PyType_Type` and relies on `PyType_Ready`
/// slot inheritance (numpy's `PyArrayDTypeMeta_Type` is the canonical case —
/// calling a DType class like `BoolDType()` dispatches
/// `Py_TYPE(cls)->tp_call`, i.e. `type.tp_call`) can instantiate its
/// instances. A canonical managed Type view instead delegates to the runtime
/// call authority; its physical `PyType_Type` carrier is not an independent
/// `tp_new` implementation.
pub unsafe extern "C" fn molt_type_call(
    callable: *mut PyObject,
    args: *mut PyObject,
    kwds: *mut PyObject,
) -> *mut PyObject {
    let tp = callable.cast::<PyTypeObject>();
    if tp.is_null() {
        return ptr::null_mut();
    }
    let type_type = &raw mut crate::abi_types::PyType_Type;
    unsafe {
        // Special case: type(x) should return Py_TYPE(x). Only `type` itself
        // accepts the one-argument form (#27157).
        if ptr::eq(tp, type_type) {
            let nargs = if args.is_null() {
                0
            } else {
                crate::api::sequences::PyTuple_Size(args)
            };
            let kwds_empty = kwds.is_null() || crate::api::mapping::PyDict_Size(kwds) == 0;
            if nargs == 1 && kwds_empty {
                let item = crate::api::sequences::PyTuple_GetItem(args, 0);
                if item.is_null() {
                    return ptr::null_mut();
                }
                let item_type = crate::bridge::semantic_type(item).cast::<PyObject>();
                if item_type.is_null() {
                    return ptr::null_mut();
                }
                crate::api::refcount::Py_INCREF(item_type);
                return item_type;
            }
            if nargs != 3 {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"type() takes 1 or 3 arguments".as_ptr(),
                );
                return ptr::null_mut();
            }
        }

        if crate::api::object::runtime_call_authority(callable, true) {
            return crate::api::object::call_managed_callable(callable, args, kwds);
        }

        let Some(tp_new) = (*tp).tp_new else {
            let name = if (*tp).tp_name.is_null() {
                "<anonymous>".to_string()
            } else {
                std::ffi::CStr::from_ptr((*tp).tp_name)
                    .to_string_lossy()
                    .into_owned()
            };
            crate::capi_trace::record_silent_failure("type_call", Some(&name));
            if let Ok(msg) = std::ffi::CString::new(format!("cannot create '{name}' instances")) {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    msg.as_ptr(),
                );
            }
            return ptr::null_mut();
        };

        // Env-gated diagnostic (MOLT_TRACE_CAPI): name the type being
        // instantiated, its `tp_new` slot pointer, and whether the call arrived
        // with a NULL args pointer. This is the probe that pins split-runtime
        // DType-instantiation failures (numpy `use_new_as_default`) to a
        // concrete DType + slot. Zero cost when the env var is unset.
        if crate::capi_trace::trace_enabled() {
            let name = if (*tp).tp_name.is_null() {
                "<anonymous>".to_string()
            } else {
                std::ffi::CStr::from_ptr((*tp).tp_name)
                    .to_string_lossy()
                    .into_owned()
            };
            crate::capi_trace::trace_call(
                "molt_type_call:new",
                Some(&format!(
                    "{name} tp_new={:p} args_null={} kwds_null={}",
                    tp_new as *const (),
                    args.is_null(),
                    kwds.is_null()
                )),
            );
        }

        let obj = tp_new(tp, args, kwds);

        if crate::capi_trace::trace_enabled() {
            let result_type = if obj.is_null() {
                "NULL".to_string()
            } else if (*obj).ob_type.is_null() || (*(*obj).ob_type).tp_name.is_null() {
                "<anonymous-result>".to_string()
            } else {
                std::ffi::CStr::from_ptr((*(*obj).ob_type).tp_name)
                    .to_string_lossy()
                    .into_owned()
            };
            crate::capi_trace::trace_call(
                "molt_type_call:result",
                Some(&format!("-> {result_type}")),
            );
        }
        // _Py_CheckFunctionResult: fail closed on a contract violation rather
        // than silently propagating a bare NULL / stale exception.
        if obj.is_null() {
            if crate::api::errors::PyErr_Occurred().is_null() {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"tp_new returned NULL without setting an exception".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        if !crate::api::errors::PyErr_Occurred().is_null() {
            crate::api::refcount::Py_DECREF(obj);
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_SystemError).cast::<crate::abi_types::PyObject>(),
                c"tp_new returned a result with an exception set".as_ptr(),
            );
            return ptr::null_mut();
        }

        // If the returned object is not an instance of the called type, it
        // won't be initialized.
        if PyObject_TypeCheck(obj, tp) == 0 {
            return obj;
        }

        let instance_type = (*obj).ob_type;
        if !instance_type.is_null()
            && let Some(tp_init) = (*instance_type).tp_init
        {
            let res = tp_init(obj, args, kwds);
            if res < 0 {
                if crate::api::errors::PyErr_Occurred().is_null() {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"tp_init failed without setting an exception".as_ptr(),
                    );
                }
                crate::api::refcount::Py_DECREF(obj);
                return ptr::null_mut();
            }
        }
        obj
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GenericAlloc(
    tp: *mut PyTypeObject,
    nitems: Py_ssize_t,
) -> *mut PyObject {
    unsafe {
        let object = crate::api::memory::molt_object_alloc_with_tail(tp, nitems, true);
        if object.is_null() || (*tp).tp_flags & Py_TPFLAGS_HAVE_GC == 0 {
            return object;
        }
        if (crate::hooks::hooks_or_stubs().native_gc_track)(object.addr()) < 0 {
            crate::api::errors::check_native_status(-1, "PyType_GenericAlloc GC publication");
            crate::api::errors::with_preserved_error(|| {
                // Storage is initialized, payload construction has not started.
                let heap_type = (*tp).tp_flags & Py_TPFLAGS_HEAPTYPE != 0;
                if let Some(storage) = NativeDeallocation::storage(object) {
                    storage.finish();
                }
                if heap_type {
                    crate::api::refcount::Py_DECREF(tp.cast());
                }
            });
            ptr::null_mut()
        } else {
            object
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GenericNew(
    tp: *mut PyTypeObject,
    _args: *mut PyObject,
    _kwds: *mut PyObject,
) -> *mut PyObject {
    // CPython Objects/typeobject.c: `return type->tp_alloc(type, 0);` — dispatch
    // the type's OWN tp_alloc slot (a C extension may install a custom allocator).
    // Fall back to PyType_GenericAlloc only when tp_alloc is absent.
    if !tp.is_null()
        && let Some(alloc) = unsafe { (*tp).tp_alloc }
    {
        return unsafe { alloc(tp, 0) };
    }
    unsafe { PyType_GenericAlloc(tp, 0) }
}

use crate::type_slots as ts;

/// Apply every entry of a `PyType_Spec.slots` array (terminated by `slot == 0`)
/// to the corresponding field of the type under construction. Mirrors the slot
/// dispatch of CPython 3.12's `PyType_FromMetaclass` (`Objects/typeobject.c`):
/// each `Py_tp_*` id targets a `tp_*` field, each `Py_nb_*/sq_*/mp_*/am_*/bf_*`
/// id targets the admitted inline protocol sub-table, and `Py_tp_doc` copies the
/// documentation string into freshly allocated memory. An unrecognised slot id
/// fails closed with a set exception (CPython raises `RuntimeError: invalid slot
/// offset`) rather than silently dropping behaviour. Returns 0 on success, -1
/// with a recorded silent failure + pending exception otherwise.
unsafe fn apply_spec_slots(
    ty: *mut PyTypeObject,
    slots: *mut crate::abi_types::PyType_Slot,
) -> c_int {
    if slots.is_null() {
        return 0;
    }
    unsafe {
        let mut slot = slots;
        while (*slot).slot != 0 {
            let id = (*slot).slot;
            // These declarations are normalized once by the construction
            // transaction before allocator callbacks. No second ownership lane.
            if matches!(id, ts::Py_tp_base | ts::Py_tp_bases | ts::Py_tp_members) {
                slot = slot.add(1);
                continue;
            }
            let pfunc = (*slot).pfunc;
            let Some(wrapper) = stable_slot_wrapper(id) else {
                crate::capi_trace::record_silent_failure(
                    "PyType_FromSpec",
                    Some(&format!("unknown PyType_Slot id {id}")),
                );
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_RuntimeError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"PyType_FromSpec: invalid slot offset".as_ptr(),
                );
                return -1;
            };
            let storage = slot_wrapper_storage(ty, wrapper);
            if storage.is_null() {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                    c"PyType_FromSpec: stable type-slot storage unavailable".as_ptr(),
                );
                return -1;
            }
            // CPython owns a private copy of tp_doc. Every other stable slot is
            // a pointer-sized value written through the shared slot-storage map.
            let stored = if id == ts::Py_tp_doc && !pfunc.is_null() {
                let src = pfunc.cast::<c_char>();
                let bytes = std::ffi::CStr::from_ptr(src).to_bytes_with_nul();
                let buf = crate::api::memory::PyMem_Malloc(bytes.len()).cast::<c_char>();
                if buf.is_null() {
                    crate::capi_trace::record_silent_failure(
                        "PyType_FromSpec",
                        Some("tp_doc allocation failed"),
                    );
                    crate::api::errors::PyErr_NoMemory();
                    return -1;
                }
                ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast::<u8>(), bytes.len());
                buf.cast::<c_void>()
            } else {
                pfunc
            };
            if id == ts::Py_tp_doc {
                crate::api::memory::PyMem_Free(storage.read());
            }
            storage.write(stored);
            slot = slot.add(1);
        }
        0
    }
}

/// Normalize the public argument and spec base declarations into one owner.
/// Metaclass selection, physical-base selection and readiness consume this tuple.
unsafe fn spec_bases(
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
) -> crate::api::refcount::OwnedPyObject {
    use crate::api::refcount::OwnedPyObject;
    use crate::api::sequences::{PyTuple_Check, PyTuple_New, PyTuple_SetItem};
    unsafe {
        let mut base = (&raw mut crate::abi_types::PyBaseObject_Type).cast::<PyObject>();
        let mut declared = ptr::null_mut::<PyObject>();
        if bases.is_null() {
            let mut slot = (*spec).slots;
            while !slot.is_null() && (*slot).slot != 0 {
                match (*slot).slot {
                    ts::Py_tp_base => base = (*slot).pfunc.cast(),
                    ts::Py_tp_bases => declared = (*slot).pfunc.cast(),
                    _ => (),
                }
                slot = slot.add(1);
            }
            if !declared.is_null() {
                if PyTuple_Check(declared) == 0 {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                        c"Py_tp_bases is not a tuple".as_ptr(),
                    );
                    return OwnedPyObject::from_owned(ptr::null_mut());
                }
                return OwnedPyObject::from_borrowed(declared);
            }
        } else if PyTuple_Check(bases) != 0 {
            return OwnedPyObject::from_borrowed(bases);
        } else {
            base = bases;
        }
        let tuple = OwnedPyObject::from_owned(PyTuple_New(1));
        if tuple.as_ptr().is_null() {
            return tuple;
        }
        crate::api::refcount::Py_XINCREF(base);
        if PyTuple_SetItem(tuple.as_ptr(), 0, base) < 0 {
            return OwnedPyObject::from_owned(ptr::null_mut());
        }
        tuple
    }
}

unsafe fn preflight_spec_slots(
    spec: *mut PyType_Spec,
) -> Option<(*mut crate::abi_types::PyMemberDef, Py_ssize_t)> {
    unsafe {
        let mut members = ptr::null_mut();
        let mut count = 0isize;
        let mut seen_members = false;
        let mut seen_doc = false;
        let mut slot = (*spec).slots;
        while !slot.is_null() && (*slot).slot != 0 {
            if stable_slot_wrapper((*slot).slot).is_none() {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_RuntimeError).cast(),
                    c"PyType_FromSpec: invalid slot offset".as_ptr(),
                );
                return None;
            }
            match (*slot).slot {
                ts::Py_tp_members => {
                    if seen_members || (*slot).pfunc.is_null() {
                        crate::api::errors::PyErr_SetString(
                            (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                            c"Py_tp_members requires one non-null declaration".as_ptr(),
                        );
                        return None;
                    }
                    seen_members = true;
                    members = (*slot).pfunc.cast::<crate::abi_types::PyMemberDef>();
                    let mut member = members;
                    while !(*member).name.is_null() {
                        if (*member).flags & crate::abi_types::Py_RELATIVE_OFFSET != 0
                            && ((*spec).basicsize >= 0
                                || (*member).offset < 0
                                || i64::try_from((*member).offset).unwrap_or(i64::MAX)
                                    >= -i64::from((*spec).basicsize))
                        {
                            crate::api::errors::PyErr_SetString((&raw mut crate::abi_types::PyExc_SystemError).cast(), c"relative member offset requires negative basicsize and an in-range offset".as_ptr());
                            return None;
                        }
                        count = count.checked_add(1).or_else(|| {
                            crate::api::errors::PyErr_NoMemory();
                            None
                        })?;
                        member = member.add(1);
                    }
                }
                ts::Py_tp_doc => {
                    if seen_doc {
                        crate::api::errors::PyErr_SetString(
                            (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                            c"multiple Py_tp_doc slots are not supported".as_ptr(),
                        );
                        return None;
                    }
                    seen_doc = true;
                }
                _ => (),
            }
            slot = slot.add(1);
        }
        Some((members, count))
    }
}

// CPython's relative type-data contract uses the target C max_align_t.
#[cfg(all(windows, target_env = "msvc"))]
const TYPE_DATA_ALIGNMENT: Py_ssize_t = std::mem::align_of::<f64>() as Py_ssize_t;
#[cfg(not(all(windows, target_env = "msvc")))]
const TYPE_DATA_ALIGNMENT: Py_ssize_t = std::mem::align_of::<libc::max_align_t>() as Py_ssize_t;

fn align_type_data(size: Py_ssize_t) -> Option<Py_ssize_t> {
    size.checked_add(TYPE_DATA_ALIGNMENT - 1)
        .map(|size| size & !(TYPE_DATA_ALIGNMENT - 1))
}

/// Shared body for `PyType_FromSpec*` / `PyType_FromMetaclass`. Allocates a real
/// `PyHeapTypeObject` (NOT a bare `Box<PyTypeObject>`), sets `Py_TPFLAGS_HEAPTYPE`
/// and populates `ht_name`/`ht_qualname`/`ht_module`, so an extension's inlined
/// `((PyHeapTypeObject*)type)->ht_name`/`ht_module` reads land IN BOUNDS and the
/// per-module state a spec type carries is retained (matrix PyTypeObject #3, L3).
/// Mirrors CPython v3.12.0 `_PyType_FromMetaclass_impl` (Objects/typeobject.c):
/// `type->tp_flags = spec->flags | Py_TPFLAGS_HEAPTYPE`, `ht_name` = the segment
/// after the last '.' in `spec->name`, `ht_qualname = ht_name`, `ht_module =
/// Py_XNewRef(module)`.
///
/// Allocation uses the selected metaclass's object-domain allocator. Failed
/// construction cuts partial self cycles; completed heap types enter the same
/// mixed collector and terminal destruction authority as native instances.
unsafe fn type_from_spec_impl(
    metaclass: *mut PyTypeObject,
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
    module: *mut PyObject,
    allow_custom_new: bool,
) -> *mut PyObject {
    if spec.is_null() || unsafe { (*spec).name }.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    unsafe {
        crate::bridge::molt_cpython_abi_init();
        let Some((members, member_count)) = preflight_spec_slots(spec) else {
            return ptr::null_mut();
        };
        // Custom metaclass allocation may reenter C. Own the declaration facts
        // before callbacks; descriptors finally borrow the heap type's copy.
        let mut member_records = Vec::new();
        if member_records
            .try_reserve_exact(member_count as usize)
            .is_err()
        {
            return crate::api::errors::PyErr_NoMemory();
        }
        for index in 0..member_count {
            member_records.push(ptr::read(members.offset(index)));
        }
        let bases = spec_bases(spec, bases);
        if bases.as_ptr().is_null() {
            return ptr::null_mut();
        }
        let count = crate::api::sequences::PyTuple_Size(bases.as_ptr());
        if count <= 0 {
            if count == 0 {
                reject_type_layout(c"bases tuple must not be empty");
            }
            return ptr::null_mut();
        }
        let mut metaclass = if metaclass.is_null() {
            &raw mut crate::abi_types::PyType_Type
        } else {
            metaclass
        };
        if PyType_Check(metaclass.cast()) == 0
            || PyType_IsSubtype(metaclass, &raw mut crate::abi_types::PyType_Type) == 0
        {
            reject_type_layout(c"metaclass must be a subtype of type");
            return ptr::null_mut();
        }
        // Select by canonical subtype relations, including every explicit base.
        let mut consider = |base: *mut PyTypeObject| -> bool {
            if base.is_null() || PyType_Check(base.cast()) == 0 {
                reject_type_layout(c"bases must contain only type objects");
                return false;
            }
            let candidate = crate::bridge::semantic_type(base.cast());
            if candidate.is_null() {
                return false;
            }
            if PyType_IsSubtype(metaclass, candidate) != 0 {
                return true;
            }
            if PyType_IsSubtype(candidate, metaclass) != 0 {
                metaclass = candidate;
                return true;
            }
            reject_type_layout(c"metaclass conflict among bases");
            false
        };
        for index in 0..count {
            if !consider(crate::api::sequences::PyTuple_GetItem(bases.as_ptr(), index).cast()) {
                return ptr::null_mut();
            }
        }
        let _metaclass_owner = crate::api::refcount::OwnedPyObject::from_borrowed(metaclass.cast());
        if PyType_Ready(metaclass) < 0 {
            return ptr::null_mut();
        }
        if (*metaclass).tp_basicsize < std::mem::size_of::<PyHeapTypeObject>() as Py_ssize_t
            || (*metaclass).tp_itemsize
                != std::mem::size_of::<crate::abi_types::PyMemberDef>() as Py_ssize_t
        {
            reject_type_layout(c"metaclass storage cannot hold a heap type");
            return ptr::null_mut();
        }
        if let Some(new) = (*metaclass).tp_new
            && crate::abi_types::PyType_Type
                .tp_new
                .is_none_or(|canonical| !ptr::fn_addr_eq(new, canonical))
        {
            if !allow_custom_new {
                reject_type_layout(c"metaclasses with custom tp_new are not supported");
                return ptr::null_mut();
            }
            if crate::api::errors::PyErr_WarnEx(
                (&raw mut crate::abi_types::PyExc_DeprecationWarning).cast(),
                c"PyType_Spec with a metaclass that has custom tp_new is deprecated".as_ptr(),
                1,
            ) < 0
            {
                return ptr::null_mut();
            }
        }
        let base = acceptable_best_base(bases.as_ptr());
        if base.is_null() {
            return ptr::null_mut();
        }
        let _base_owner = crate::api::refcount::OwnedPyObject::from_borrowed(base.cast());
        let mut basicsize = (*spec).basicsize as Py_ssize_t;
        let mut data_offset = basicsize;
        if basicsize == 0 {
            basicsize = (*base).tp_basicsize;
        } else if basicsize < 0 {
            if (*base).tp_itemsize != 0
                && ((*base).tp_flags | (*spec).flags as std::os::raw::c_ulong)
                    & crate::abi_types::Py_TPFLAGS_ITEMS_AT_END
                    == 0
            {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                    c"cannot extend a variable-size class without Py_TPFLAGS_ITEMS_AT_END".as_ptr(),
                );
                return ptr::null_mut();
            }
            let Some(offset) = align_type_data((*base).tp_basicsize) else {
                return crate::api::errors::PyErr_NoMemory();
            };
            data_offset = offset;
            let Some(size) = basicsize
                .checked_neg()
                .and_then(align_type_data)
                .and_then(|extra| offset.checked_add(extra))
            else {
                return crate::api::errors::PyErr_NoMemory();
            };
            basicsize = size;
        }
        if (*spec).itemsize < 0
            || basicsize < (*base).tp_basicsize
            || ((*spec).itemsize != 0
                && (*base).tp_itemsize != 0
                && (*spec).itemsize as Py_ssize_t != (*base).tp_itemsize)
        {
            reject_type_layout(c"type spec sizes are incompatible with its base layout");
            return ptr::null_mut();
        }
        // Capture the allocating metaclass request before any allocator callback.
        // Later mutations cannot enlarge this operation's minimum extent.
        let member_offset = (*metaclass).tp_basicsize as usize;
        let Some(required_bytes) =
            crate::api::memory::native_object_layout_size(metaclass, member_count, true)
        else {
            return crate::api::errors::PyErr_NoMemory();
        };
        let allocate = (*metaclass).tp_alloc.unwrap_or(PyType_GenericAlloc);
        let owns_metaclass = (*metaclass).tp_flags & Py_TPFLAGS_HEAPTYPE != 0;
        if let Some(reason) = crate::api::memory::native_gc_type_admission_error(metaclass) {
            crate::api::memory::raise_native_gc_admission_error(reason);
            return ptr::null_mut();
        }
        let object = allocate(metaclass, member_count);
        if object.is_null() {
            return crate::api::errors::check_native_result(object, "heap type allocation");
        }
        // Capture the actual initialized header owner before validating the
        // requested contract. Rejection cannot release an unacquired owner or
        // overwrite the allocator's own exception with a storage-size error.
        let allocated_class = (*object).ob_type;
        let owns_allocated_class = if allocated_class == metaclass {
            owns_metaclass
        } else {
            !allocated_class.is_null() && (*allocated_class).tp_flags & Py_TPFLAGS_HEAPTYPE != 0
        };
        if crate::api::errors::check_native_status(0, "heap type allocation") < 0 {
            native_lifecycle::release_unconstructed_type(
                object,
                allocated_class,
                owns_allocated_class,
            );
            return ptr::null_mut();
        }
        if allocated_class != metaclass {
            reject_type_layout(c"type allocator returned an object with the wrong metaclass");
            native_lifecycle::release_unconstructed_type(
                object,
                allocated_class,
                owns_allocated_class,
            );
            return ptr::null_mut();
        }
        if let Err(error) = admit_spec_type_allocation(object.addr(), required_bytes) {
            match error {
                TypeStorageAdmissionError::InsufficientExtent => {
                    reject_type_layout(c"type allocator returned insufficient heap storage");
                }
                TypeStorageAdmissionError::Capacity => {
                    crate::api::errors::PyErr_NoMemory();
                }
            }
            // Only the compact initialized object header is admitted here.
            // Release the allocator's storage without invoking type_dealloc,
            // which cannot read an unproven tail.
            native_lifecycle::release_unconstructed_type(object, metaclass, owns_metaclass);
            return ptr::null_mut();
        }
        if let Some(reason) = crate::api::memory::native_gc_type_admission_error(metaclass) {
            crate::api::memory::raise_native_gc_admission_error(reason);
            native_lifecycle::release_unconstructed_type(object, metaclass, owns_metaclass);
            return ptr::null_mut();
        }
        // Readiness publishes self/MRO/descriptor edges through the runtime.
        // A custom allocator may not have enrolled this physical allocation.
        // Admit it before the first such crossing, but keep it untracked until
        // the completed type can expose all of its initialized owned fields.
        if (crate::hooks::hooks_or_stubs().native_gc_allocate)(object.addr()) < 0 {
            crate::api::errors::check_native_status(-1, "heap type GC admission");
            native_lifecycle::release_unconstructed_type(object, metaclass, owns_metaclass);
            return ptr::null_mut();
        }
        crate::api::memory::PyObject_GC_UnTrack(object.cast());
        let heap_ptr = object.cast::<PyHeapTypeObject>();
        let tp: *mut PyTypeObject = &raw mut (*heap_ptr).ht_type;
        (*tp).tp_flags = ((*spec).flags as std::os::raw::c_ulong & !Py_TPFLAGS_READY)
            | crate::abi_types::Py_TPFLAGS_HEAPTYPE;
        let construction = native_lifecycle::HeapTypeConstruction(
            crate::api::refcount::OwnedPyObject::from_owned(object),
        );
        crate::api::refcount::Py_INCREF(base.cast());
        (*tp).tp_base = base;
        (*tp).tp_bases = bases.into_ptr();
        let name_bytes = std::ffi::CStr::from_ptr((*spec).name).to_bytes_with_nul();
        let name = crate::api::memory::PyMem_Malloc(name_bytes.len()).cast::<c_char>();
        if name.is_null() {
            return crate::api::errors::PyErr_NoMemory();
        }
        ptr::copy_nonoverlapping(name_bytes.as_ptr(), name.cast(), name_bytes.len());
        (*heap_ptr)._ht_tpname = name;
        (*tp).tp_name = name;
        (*tp).tp_as_async = (&raw mut (*heap_ptr).as_async).cast();
        (*tp).tp_as_number = (&raw mut (*heap_ptr).as_number).cast();
        (*tp).tp_as_sequence = (&raw mut (*heap_ptr).as_sequence).cast();
        (*tp).tp_as_mapping = (&raw mut (*heap_ptr).as_mapping).cast();
        (*tp).tp_as_buffer = (&raw mut (*heap_ptr).as_buffer).cast();
        (*tp).tp_basicsize = basicsize;
        (*tp).tp_itemsize = (*spec).itemsize as Py_ssize_t;
        // ht_name / ht_qualname: the `spec->name` segment after the last '.', as a
        // str object. The C string is null-terminated, so the after-dot pointer is
        // itself a valid C string. A failed name allocation aborts construction.
        let name_ptr = name;
        if !name_ptr.is_null() {
            let short = match std::ffi::CStr::from_ptr(name_ptr)
                .to_bytes()
                .iter()
                .rposition(|&b| b == b'.')
            {
                Some(dot) => name_ptr.add(dot + 1),
                None => name_ptr,
            };
            let ht_name = crate::api::strings::PyUnicode_FromString(short);
            if ht_name.is_null() {
                return ptr::null_mut();
            }
            (*heap_ptr).ht_name = ht_name;
            crate::api::refcount::Py_INCREF(ht_name);
            (*heap_ptr).ht_qualname = ht_name;
        }

        // ht_module: retain the defining module (Py_XNewRef) so PyType_GetModule /
        // PyType_GetModuleState resolve instead of dropping per-module state.
        if !module.is_null() {
            crate::api::refcount::Py_INCREF(module);
            (*heap_ptr).ht_module = module;
        }

        // Member descriptors retain their declaration pointer. Move the records
        // into the metaclass allocation's variable tail, including its sentinel.
        if !members.is_null() {
            let destination = object
                .cast::<u8>()
                .add(member_offset)
                .cast::<crate::abi_types::PyMemberDef>();
            ptr::copy_nonoverlapping(member_records.as_ptr(), destination, member_count as usize);
            destination
                .add(member_count as usize)
                .write(std::mem::zeroed());
            for index in 0..member_count {
                let member = &mut *destination.offset(index);
                if member.flags & crate::abi_types::Py_RELATIVE_OFFSET != 0 {
                    member.flags &= !crate::abi_types::Py_RELATIVE_OFFSET;
                    member.offset += data_offset;
                }
                let name = std::ffi::CStr::from_ptr(member.name);
                let offset = if name == c"__weaklistoffset__" {
                    Some(&raw mut (*tp).tp_weaklistoffset)
                } else if name == c"__dictoffset__" {
                    Some(&raw mut (*tp).tp_dictoffset)
                } else if name == c"__vectorcalloffset__" {
                    Some(&raw mut (*tp).tp_vectorcall_offset)
                } else {
                    None
                };
                if let Some(offset) = offset {
                    if member.type_ != PY_T_PYSSIZET || member.flags != PY_READONLY {
                        crate::api::errors::PyErr_SetString(
                            (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                            c"special offset member must be a readonly Py_ssize_t".as_ptr(),
                        );
                        return ptr::null_mut();
                    }
                    offset.write(member.offset);
                }
            }
            (*tp).tp_members = destination;
        }

        // Base and member ownership is complete; apply the ordinary slots.
        if apply_spec_slots(tp, (*spec).slots) < 0 {
            return ptr::null_mut();
        }
        native_lifecycle::prepare_heap_defaults(tp, false);
        if !(*tp).tp_base.is_null() && validate_base_layout(tp, (*tp).tp_base) < 0 {
            return ptr::null_mut();
        }

        // (3) Instantiation defaults where the spec left them unset.
        if (*tp).tp_alloc.is_none() {
            (*tp).tp_alloc = Some(PyType_GenericAlloc);
        }

        // (4) Comprehensive readiness pipeline (base default, slot inherit, dict,
        //     mro, mark READY).
        if PyType_Ready(tp) < 0 {
            return ptr::null_mut();
        }
        if (*tp).tp_alloc.is_some_and(|alloc| {
            ptr::fn_addr_eq(
                alloc,
                PyType_GenericAlloc
                    as unsafe extern "C" fn(*mut PyTypeObject, Py_ssize_t) -> *mut PyObject,
            )
        }) {
            let Some(size) = crate::api::memory::native_object_layout_size(tp, 0, false) else {
                return crate::api::errors::PyErr_NoMemory();
            };
            for offset in [
                (*tp).tp_dictoffset,
                (*tp).tp_weaklistoffset,
                (*tp).tp_vectorcall_offset,
            ] {
                if offset == 0 {
                    continue;
                }
                let position = if offset < 0 {
                    (size as isize).checked_add(offset)
                } else {
                    Some(offset)
                };
                if position.is_none_or(|position| {
                    position < std::mem::size_of::<PyObject>() as isize
                        || !(position as usize)
                            .is_multiple_of(std::mem::align_of::<*mut PyObject>())
                        || (position as usize)
                            .checked_add(std::mem::size_of::<*mut PyObject>())
                            .is_none_or(|end| end > size)
                }) {
                    reject_type_layout(
                        c"type spec offset is outside its generic allocation layout",
                    );
                    return ptr::null_mut();
                }
            }
        }
        // These two declarations configure layout; CPython removes their
        // temporary member descriptors instead of exposing raw offset reads.
        for member in &member_records {
            let name = std::ffi::CStr::from_ptr(member.name);
            if (name == c"__dictoffset__" || name == c"__weaklistoffset__")
                && crate::api::mapping::PyDict_DelItemString((*tp).tp_dict, member.name) < 0
            {
                return ptr::null_mut();
            }
        }
        if crate::api::memory::PyObject_GC_IsTracked(object) == 0 {
            crate::api::memory::PyObject_GC_Track(object.cast());
        }
        construction.into_ptr()
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_FromSpec(spec: *mut PyType_Spec) -> *mut PyObject {
    unsafe { PyType_FromSpecWithBases(spec, ptr::null_mut()) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_FromSpecWithBases(
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
) -> *mut PyObject {
    unsafe { type_from_spec_impl(ptr::null_mut(), spec, bases, ptr::null_mut(), true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_FromModuleAndSpec(
    module: *mut PyObject,
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
) -> *mut PyObject {
    unsafe { type_from_spec_impl(ptr::null_mut(), spec, bases, module, true) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_FromMetaclass(
    metaclass: *mut PyTypeObject,
    module: *mut PyObject,
    spec: *mut PyType_Spec,
    bases: *mut PyObject,
) -> *mut PyObject {
    unsafe { type_from_spec_impl(metaclass, spec, bases, module, false) }
}

/// CPython `PyType_GetModule` (Objects/typeobject.c): the module a heap type was
/// defined in. Requires `Py_TPFLAGS_HEAPTYPE` (TypeError otherwise) and reads
/// `((PyHeapTypeObject*)type)->ht_module` — in bounds now that spec types are full
/// `PyHeapTypeObject`s.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetModule(ty: *mut PyTypeObject) -> *mut PyObject {
    if ty.is_null() {
        return ptr::null_mut();
    }
    if unsafe { (*ty).tp_flags } & crate::abi_types::Py_TPFLAGS_HEAPTYPE == 0
        || heap_type_storage(ty).is_none()
    {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"PyType_GetModule: Type is not a heap type".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    let et = heap_type_storage(ty).expect("heap storage admitted above");
    let m = unsafe { (*et).ht_module };
    if m.is_null() {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                c"PyType_GetModule: This type has no module associated with it".as_ptr(),
            );
        }
        return ptr::null_mut();
    }
    m
}

/// CPython `PyType_GetModuleState`: the per-module state of the heap type's
/// defining module, or NULL (with the `PyType_GetModule` exception on a non-heap
/// type, or cleanly when the module carries no state).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetModuleState(ty: *mut PyTypeObject) -> *mut c_void {
    let m = unsafe { PyType_GetModule(ty) };
    if m.is_null() {
        return ptr::null_mut();
    }
    unsafe { crate::api::modules::PyModule_GetState(m) }
}

/// CPython `PyType_GetModuleByDef` (Objects/typeobject.c): walk `type`'s MRO and
/// return the first heap type's defining module whose exact definition is `def`.
/// This is a borrowed reference. Multi-phase modules are not members of the
/// single-phase PyState registry, and secondary bases participate in MRO order.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetModuleByDef(
    ty: *mut PyTypeObject,
    def: *mut crate::abi_types::PyModuleDef,
) -> *mut PyObject {
    if ty.is_null() || def.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    unsafe {
        if (*ty).tp_mro.is_null() && PyType_Ready(ty) < 0 {
            return ptr::null_mut();
        }
        let mro = crate::api::refcount::OwnedPyObject::from_borrowed((*ty).tp_mro);
        let count = crate::api::sequences::PyTuple_Size(mro.as_ptr());
        if count < 0 {
            return ptr::null_mut();
        }
        for index in 0..count {
            let base =
                crate::api::sequences::PyTuple_GetItem(mro.as_ptr(), index).cast::<PyTypeObject>();
            if base.is_null() {
                return ptr::null_mut();
            }
            if (*base).tp_flags & Py_TPFLAGS_HEAPTYPE == 0 {
                continue;
            }
            let Some(heap) = heap_type_storage(base) else {
                continue;
            };
            let module = (*heap).ht_module;
            if module.is_null() {
                continue;
            }
            let definition = crate::api::modules::PyModule_GetDef(module);
            if definition == def {
                return module;
            }
            if definition.is_null() && descriptors::pending() {
                return ptr::null_mut();
            }
        }
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_TypeError).cast(),
            c"PyType_GetModuleByDef: no superclass defines the requested module".as_ptr(),
        );
    }
    ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    // CPython: PyType_Check(op) == PyType_FastSubclass(Py_TYPE(op),
    // Py_TPFLAGS_TYPE_SUBCLASS) — true whenever op's METATYPE is `type` OR a
    // subtype of it (numpy DType classes carry a `type`-subclass metatype such
    // as `PyArrayDTypeMeta_Type`). The prior exact ob_type == &PyType_Type
    // compare answered only PyType_CheckExact and rejected every C metaclass
    // instance. Walk the metatype's subtype chain like PyObject_TypeCheck.
    let type_type = &raw mut crate::abi_types::PyType_Type;
    let meta = unsafe { crate::bridge::semantic_type(op) };
    if meta.is_null() {
        return 0;
    }
    if std::ptr::eq(meta, type_type) {
        return 1;
    }
    unsafe { PyType_IsSubtype(meta, type_type) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Modified(tp: *mut PyTypeObject) {
    unsafe fn invalidate_one(identity: TypeIdentity) {
        let Some(tp) = (unsafe { identity.live_type(&TYPE_SUBCLASSES.lock()) }) else {
            return;
        };
        let watched = unsafe { (*tp).tp_watched };
        // Retention admits the lifetime before any field writes. Watched types
        // also need that owner across callbacks and unraisable reporting,
        // outside the registry lock.
        // A managed view's teardown pin can keep its C header positive after
        // runtime death is committed. Checked retention rejects that window
        // before either watcher callbacks or version-flag writes.
        let Some(_owner) =
            (unsafe { crate::api::refcount::OwnedPyObject::try_from_borrowed(tp.cast()) })
        else {
            return;
        };
        if watched != 0 {
            for watcher_id in 0..TYPE_MAX_WATCHERS {
                if watched & (1 << watcher_id) == 0 {
                    continue;
                }
                // A prior callback may clear or replace a later slot. Keep the
                // watched-bit snapshot, but resolve each callback at dispatch.
                let callback = {
                    let watcher_state = TYPE_WATCHER_STATE.lock();
                    debug_assert_eq!(watcher_state.interpreter_id, CANONICAL_INTERPRETER_ID);
                    watcher_state.callbacks[watcher_id]
                };
                if let Some(callback) = callback {
                    if !identity.is_live(&TYPE_SUBCLASSES.lock()) {
                        return;
                    }
                    crate::api::errors::with_preserved_error(|| unsafe {
                        let status = callback(tp.cast::<PyObject>());
                        if status < 0 && !crate::api::errors::raised_error_pending() {
                            crate::api::errors::PyErr_SetString(
                                (&raw mut crate::abi_types::PyExc_SystemError).cast(),
                                c"type watcher callback failed without an exception".as_ptr(),
                            );
                        }
                        if crate::api::errors::raised_error_pending() {
                            let context = if identity.is_live(&TYPE_SUBCLASSES.lock()) {
                                tp.cast()
                            } else {
                                ptr::null_mut()
                            };
                            crate::api::errors::PyErr_WriteUnraisable(context);
                        }
                    });
                }
            }
            if !identity.is_live(&TYPE_SUBCLASSES.lock()) {
                return;
            }
        }
        unsafe {
            (*tp).tp_flags &= !crate::abi_types::Py_TPFLAGS_VALID_VERSION_TAG;
            (*tp).tp_version_tag = 0;
            if let Some(heap) = heap_type_storage(tp) {
                (*heap)._spec_cache.getitem = ptr::null_mut();
            }
        }
    }
    if tp.is_null() {
        return;
    }
    // Serialize lifetime observation and retaining a live type through the
    // existing runtime token. Never take a bridge/refcount lock under the
    // subclass registry lock.
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    let Some(root) = type_identity(&mut TYPE_SUBCLASSES.lock(), tp) else {
        return;
    };
    // Explicit post-order traversal preserves CPython's subclass-before-base
    // callback order without consuming one Rust stack frame per hierarchy
    // level. Children are pushed in reverse so their registration order is
    // observed deterministically.
    let mut seen = HashSet::new();
    let mut work = vec![(root, false)];
    while let Some((current, expanded)) = work.pop() {
        if expanded {
            unsafe { invalidate_one(current) };
            continue;
        }
        let mut registry = TYPE_SUBCLASSES.lock();
        let Some(pointer) = (unsafe { current.live_type(&registry) }) else {
            // Live subclasses retain their bases, so a dying type cannot
            // have a live subtree. Keep registration for possible resurrection.
            continue;
        };
        if !seen.insert(current) {
            continue;
        }
        if unsafe { (*pointer).tp_flags } & crate::abi_types::Py_TPFLAGS_VALID_VERSION_TAG == 0 {
            continue;
        }
        work.push((current, true));
        if let Some(entry) = registry.subclasses.get_mut(&current) {
            // Copy identities directly into the traversal queue under the
            // membership lock, without a temporary allocation per parent.
            entry.compact();
            work.extend(entry.order.iter().rev().map(|&child| (child, false)));
        }
    }
}

unsafe fn validate_type_watcher_id(watcher_id: c_int) -> bool {
    if watcher_id < 0 || watcher_id as usize >= TYPE_MAX_WATCHERS {
        unsafe {
            crate::api::errors::PyErr_Format(
                (&raw mut crate::abi_types::PyExc_ValueError).cast(),
                c"Invalid type watcher ID %d".as_ptr(),
                watcher_id,
            )
        };
        return false;
    }
    if TYPE_WATCHER_STATE.lock().callbacks[watcher_id as usize].is_none() {
        unsafe {
            crate::api::errors::PyErr_Format(
                (&raw mut crate::abi_types::PyExc_ValueError).cast(),
                c"No type watcher set for ID %d".as_ptr(),
                watcher_id,
            )
        };
        return false;
    }
    true
}

unsafe fn assign_type_version_tag(tp: *mut PyTypeObject, seen: &mut HashSet<usize>) -> bool {
    // Already-admitted lookups do not execute the fallible base walk and need
    // no exception snapshot or cleanup transaction.
    if !tp.is_null()
        && unsafe { (*tp).tp_flags } & crate::abi_types::Py_TPFLAGS_VALID_VERSION_TAG != 0
    {
        return true;
    }
    // Tag admission is best effort, including malformed physical base tuples.
    // A failed admission must not turn a successful watch/lookup into a
    // success-with-error result or replace a pre-existing exception.
    crate::api::errors::with_preserved_error(|| unsafe { assign_type_version_tag_inner(tp, seen) })
}

unsafe fn assign_type_version_tag_inner(tp: *mut PyTypeObject, seen: &mut HashSet<usize>) -> bool {
    if tp.is_null() {
        return false;
    }
    if unsafe { (*tp).tp_flags } & crate::abi_types::Py_TPFLAGS_VALID_VERSION_TAG != 0 {
        return true;
    }
    if TYPE_SUBCLASSES.lock().retired_statics.contains(&tp.addr()) {
        return false;
    }
    if !seen.insert(tp as usize) {
        return false;
    }
    if unsafe { (*tp).tp_flags } & Py_TPFLAGS_READY == 0 {
        return false;
    }
    let bases = unsafe { (*tp).tp_bases };
    if !bases.is_null() {
        let count = unsafe { crate::api::sequences::PyTuple_Size(bases) };
        if count < 0 {
            return false;
        }
        for index in 0..count {
            let base = unsafe { crate::api::sequences::PyTuple_GetItem(bases, index) }
                .cast::<PyTypeObject>();
            if !unsafe { assign_type_version_tag_inner(base, seen) } {
                return false;
            }
        }
    }
    let Some(tag) = allocate_type_version_tag(&NEXT_TYPE_VERSION_TAG) else {
        return false;
    };
    unsafe {
        (*tp).tp_version_tag = tag;
        (*tp).tp_flags |= crate::abi_types::Py_TPFLAGS_VALID_VERSION_TAG;
    }
    true
}

fn allocate_type_version_tag(counter: &AtomicU32) -> Option<u32> {
    match counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        current.checked_add(1)
    }) {
        Ok(tag) if tag != 0 => Some(tag),
        _ => None,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_AddWatcher(callback: Option<PyTypeWatchCallback>) -> c_int {
    let Some(callback) = callback else {
        unsafe { reject_type_layout(c"type watcher callback must not be NULL") };
        return -1;
    };
    let mut watcher_state = TYPE_WATCHER_STATE.lock();
    debug_assert_eq!(watcher_state.interpreter_id, CANONICAL_INTERPRETER_ID);
    if let Some((index, slot)) = watcher_state
        .callbacks
        .iter_mut()
        .enumerate()
        .find(|(_, slot)| slot.is_none())
    {
        *slot = Some(callback);
        return index as c_int;
    }
    drop(watcher_state);
    unsafe {
        crate::api::errors::PyErr_SetString(
            (&raw mut crate::abi_types::PyExc_RuntimeError).cast(),
            c"no more type watcher IDs available".as_ptr(),
        )
    };
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_ClearWatcher(watcher_id: c_int) -> c_int {
    if !unsafe { validate_type_watcher_id(watcher_id) } {
        return -1;
    }
    TYPE_WATCHER_STATE.lock().callbacks[watcher_id as usize] = None;
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Watch(watcher_id: c_int, obj: *mut PyObject) -> c_int {
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    if obj.is_null() || unsafe { PyType_Check(obj) } == 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_ValueError).cast(),
                c"Cannot watch non-type".as_ptr(),
            )
        };
        return -1;
    }
    if !unsafe { validate_type_watcher_id(watcher_id) } {
        return -1;
    }
    let tp = obj.cast::<PyTypeObject>();
    // Tag exhaustion or an unready type does not reject watcher registration.
    // A later cacheable lookup can assign the tag, as in CPython.
    unsafe { assign_type_version_tag(tp, &mut HashSet::new()) };
    unsafe { (*tp).tp_watched |= 1 << watcher_id };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_Unwatch(watcher_id: c_int, obj: *mut PyObject) -> c_int {
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    if obj.is_null() || unsafe { PyType_Check(obj) } == 0 {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_ValueError).cast(),
                c"Cannot watch non-type".as_ptr(),
            )
        };
        return -1;
    }
    if !unsafe { validate_type_watcher_id(watcher_id) } {
        return -1;
    }
    unsafe { (*obj.cast::<PyTypeObject>()).tp_watched &= !(1 << watcher_id) };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyUnstable_Type_AssignVersionTag(tp: *mut PyTypeObject) -> c_int {
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    unsafe { assign_type_version_tag(tp, &mut HashSet::new()) as c_int }
}

/// Lookup a single physical/runtime namespace while preserving the caller's
/// C3 traversal order. The bridge returns the original descriptor, borrowed.
unsafe fn type_namespace_lookup(tp: *mut PyTypeObject, name: *mut PyObject) -> *mut PyObject {
    unsafe { type_namespace_lookup_with_bridge(&GLOBAL_BRIDGE, tp, name) }
}

unsafe fn type_namespace_lookup_with_bridge(
    bridge: &crate::bridge::ObjectBridge,
    tp: *mut PyTypeObject,
    name: *mut PyObject,
) -> *mut PyObject {
    unsafe {
        if let Some(class) = bridge.observed_handle_for_pyobj(tp.cast()) {
            let hooks = crate::hooks::hooks_or_stubs();
            if (hooks.classify_heap)(class.bits()) == crate::abi_types::MoltTypeTag::Type as u8 {
                let Some(name) = crate::bridge::RuntimeValue::acquire(name) else {
                    return ptr::null_mut();
                };
                return bridge.borrowed_result_to_borrowed_pyobj((hooks.type_lookup_borrowed)(
                    class.bits(),
                    name.bits(),
                    0,
                ));
            }
        }
        // A native namespace must expose its declarations through the same
        // lifecycle as every other type. READYING owns its partially populated
        // dictionary; reentrant lookup may inspect it but cannot start another
        // readiness transaction.
        if ((*tp).tp_dict.is_null() || (*tp).tp_flags & Py_TPFLAGS_READY == 0)
            && (*tp).tp_flags & crate::abi_types::Py_TPFLAGS_READYING == 0
            && ready_type(bridge, tp) < 0
        {
            return ptr::null_mut();
        }
        let dict = (*tp).tp_dict;
        if dict.is_null() {
            ptr::null_mut()
        } else {
            crate::api::mapping::PyDict_GetItemWithError(dict, name)
        }
    }
}

/// Resolve `name` on `tp` by walking its MRO and returning the first matching
/// `tp_dict` entry, mirroring CPython's `_PyType_Lookup`. Returns a *borrowed*
/// reference (no incref), matching the CPython contract. Static extensions rely
/// on this for method resolution — a stub that returns NULL silently breaks
/// every inherited attribute lookup on numpy's scalar types.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _PyType_Lookup(
    tp: *mut PyTypeObject,
    name: *mut PyObject,
) -> *mut PyObject {
    if tp.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    let Some(_type_owner) =
        (unsafe { crate::api::refcount::OwnedPyObject::try_from_borrowed(tp.cast()) })
    else {
        return ptr::null_mut();
    };
    let _name_owner = unsafe { crate::api::refcount::OwnedPyObject::from_borrowed(name) };
    let found = (|| unsafe {
        if let Some(class) = GLOBAL_BRIDGE.observed_handle_for_pyobj(tp.cast()) {
            let hooks = crate::hooks::hooks_or_stubs();
            if (hooks.classify_heap)(class.bits()) == crate::abi_types::MoltTypeTag::Type as u8 {
                let Some(name) = crate::bridge::RuntimeValue::acquire(name) else {
                    return ptr::null_mut();
                };
                return GLOBAL_BRIDGE.borrowed_result_to_borrowed_pyobj((hooks
                    .type_lookup_borrowed)(
                    class.bits(),
                    name.bits(),
                    1,
                ));
            }
        }
        let mro = (*tp).tp_mro;
        if !mro.is_null() {
            let _mro_owner = crate::api::refcount::OwnedPyObject::from_borrowed(mro);
            let n = crate::api::sequences::PyTuple_Size(mro);
            if n < 0 {
                return ptr::null_mut();
            }
            let mut i: Py_ssize_t = 0;
            while i < n {
                let base = crate::api::sequences::PyTuple_GetItem(mro, i).cast::<PyTypeObject>();
                if base.is_null() {
                    return ptr::null_mut();
                }
                if !base.is_null() {
                    let found = type_namespace_lookup(base, name);
                    if !found.is_null() || !crate::api::errors::PyErr_Occurred().is_null() {
                        return found;
                    }
                }
                i += 1;
            }
            return ptr::null_mut();
        }
        // No MRO computed (type not readied): fall back to a direct base-chain
        // walk so lookups still resolve.
        let mut cur = tp;
        while !cur.is_null() {
            let found = type_namespace_lookup(cur, name);
            if !found.is_null() || !crate::api::errors::PyErr_Occurred().is_null() {
                return found;
            }
            let base = (*cur).tp_base;
            if base == cur {
                break;
            }
            cur = base;
        }
        ptr::null_mut()
    })();
    if !crate::api::errors::raised_error_pending() {
        // Error-free hits and misses rearm invalidated watchers. Failed lookup
        // keeps its original exception and leaves the tag invalid.
        unsafe { assign_type_version_tag(tp, &mut HashSet::new()) };
    }
    found
}

/// `PyDescr_IsData` — a descriptor is a *data* descriptor iff its type defines
/// `tp_descr_set`. CPython keys this purely on `Py_TYPE(descr)->tp_descr_set !=
/// NULL` (not on whether the individual `PyGetSetDef` has a setter), and both
/// `PyGetSetDescr_Type` and `PyMemberDescr_Type` install a `tp_descr_set` (which
/// itself raises `AttributeError` for a read-only entry). Attribute-resolution
/// order — a data descriptor on the type wins over the instance dict — depends on
/// an honest answer here.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDescr_IsData(descr: *mut PyObject) -> c_int {
    crate::api::errors::with_preserved_error(|| unsafe {
        crate::api::descriptor::is_data(descr).unwrap_or(false) as c_int
    })
}

/// `PyDescr_NAME(descr)` — the interned attribute name of any descriptor. All
/// descriptor objects share the `PyDescrObject` header, so this reads
/// `d_common.d_name` (a borrowed reference, matching CPython's macro).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyDescr_NAME(descr: *mut PyObject) -> *mut PyObject {
    if descr.is_null() {
        return ptr::null_mut();
    }
    unsafe { (*descr.cast::<crate::abi_types::PyDescrObject>()).d_name }
}

/// Install the shared physical descriptor lifecycle and typed wrapper slots.
///
/// # Safety
/// Single-threaded ABI initialization, before extension access.
pub unsafe fn init_descriptor_slots() {
    unsafe {
        descriptors::init();
        method_descriptors::init();
        slot_wrappers::init();
    }
}

// Member type codes (CPython `Include/descrobject.h`, `Py_T_*`).
const PY_T_SHORT: c_int = 0;
const PY_T_INT: c_int = 1;
const PY_T_LONG: c_int = 2;
const PY_T_FLOAT: c_int = 3;
const PY_T_DOUBLE: c_int = 4;
const PY_T_STRING: c_int = 5;
const PY_T_OBJECT: c_int = 6;
const PY_T_CHAR: c_int = 7;
const PY_T_BYTE: c_int = 8;
const PY_T_UBYTE: c_int = 9;
const PY_T_USHORT: c_int = 10;
const PY_T_UINT: c_int = 11;
const PY_T_ULONG: c_int = 12;
const PY_T_BOOL: c_int = 14;
const PY_T_OBJECT_EX: c_int = 16;
const PY_T_LONGLONG: c_int = 17;
const PY_T_ULONGLONG: c_int = 18;
const PY_T_PYSSIZET: c_int = 19;
const PY_T_NONE: c_int = 20;
const PY_READONLY: c_int = 1;

/// `PyMember_GetOne` — read one struct member into a Python object. Faithful to
/// CPython `Python/structmember.c`. `addr` is the base address of the containing
/// object; the member lives at `addr + member->offset` with the C type given by
/// `member->type`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMember_GetOne(
    addr: *const c_char,
    member: *mut crate::abi_types::PyMemberDef,
) -> *mut PyObject {
    if addr.is_null() || member.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        let field = addr.offset((*member).offset) as *const c_void;
        // Width-correct constructors: `c_long` is 32-bit on Windows MSVC, so
        // 64-bit members must route through the LongLong/Ssize_t constructors to
        // avoid silent truncation. C `long`/`unsigned long` map to Ssize_t/Size_t
        // which are pointer-width and cover both LP64 and LLP64 hosts.
        match (*member).type_ {
            PY_T_SHORT => crate::api::numbers::PyLong_FromSsize_t(*(field as *const i16) as isize),
            PY_T_INT => crate::api::numbers::PyLong_FromSsize_t(*(field as *const i32) as isize),
            PY_T_LONG => crate::api::numbers::PyLong_FromLongLong(
                *(field as *const std::os::raw::c_long) as c_longlong,
            ),
            PY_T_FLOAT => crate::api::numbers::PyFloat_FromDouble(*(field as *const f32) as f64),
            // `field = addr + offset` is only guaranteed aligned to the C
            // object's struct alignment, which on wasm32 is 4 for a statically
            // declared object. An 8-byte `f64` read there would be a misaligned
            // dereference (UB; caught by the debug alignment check), so read it
            // unaligned. (4-byte members above are always ≥4-aligned = safe.)
            PY_T_DOUBLE => crate::api::numbers::PyFloat_FromDouble(std::ptr::read_unaligned(
                field as *const f64,
            )),
            PY_T_BOOL => {
                let b = *field.cast::<c_char>() != 0;
                let obj = if b {
                    (&raw mut crate::abi_types::Py_True).cast::<PyObject>()
                } else {
                    (&raw mut crate::abi_types::Py_False).cast::<PyObject>()
                };
                crate::api::refcount::Py_INCREF(obj);
                obj
            }
            // CPython reads `*(char *)addr`: the result follows the target's
            // `char` signedness (0..=255 on aarch64 Linux, -128..=127 elsewhere).
            PY_T_BYTE => {
                crate::api::numbers::PyLong_FromLong(c_long::from(*field.cast::<c_char>()))
            }
            PY_T_UBYTE => crate::api::numbers::PyLong_FromSize_t(*field.cast::<u8>() as usize),
            PY_T_USHORT => crate::api::numbers::PyLong_FromSize_t(*(field as *const u16) as usize),
            PY_T_UINT => crate::api::numbers::PyLong_FromSize_t(*(field as *const u32) as usize),
            PY_T_ULONG => crate::api::numbers::PyLong_FromUnsignedLongLong(
                *(field as *const std::os::raw::c_ulong) as c_ulonglong,
            ),
            // 8-byte members: `field` may be only 4-aligned (see PY_T_DOUBLE) —
            // read unaligned to avoid a misaligned dereference on wasm32.
            PY_T_LONGLONG => crate::api::numbers::PyLong_FromLongLong(std::ptr::read_unaligned(
                field as *const c_longlong,
            )),
            PY_T_ULONGLONG => crate::api::numbers::PyLong_FromUnsignedLongLong(
                std::ptr::read_unaligned(field as *const c_ulonglong),
            ),
            PY_T_PYSSIZET => crate::api::numbers::PyLong_FromSsize_t(*(field as *const isize)),
            PY_T_CHAR => {
                let c = *(field as *const c_char);
                let buf = [crate::platform::c_char_to_u8(c), 0u8];
                crate::api::strings::PyUnicode_FromStringAndSize(buf.as_ptr().cast(), 1)
            }
            PY_T_STRING => {
                let s = *(field as *const *const c_char);
                if s.is_null() {
                    let none = &raw mut crate::abi_types::Py_None;
                    crate::api::refcount::Py_INCREF(none);
                    none
                } else {
                    crate::api::strings::PyUnicode_FromString(s)
                }
            }
            PY_T_OBJECT | PY_T_OBJECT_EX => {
                let v = *(field as *const *mut PyObject);
                if v.is_null() {
                    if (*member).type_ == PY_T_OBJECT_EX {
                        crate::api::errors::PyErr_SetString(
                            (&raw mut crate::abi_types::PyExc_AttributeError)
                                .cast::<crate::abi_types::PyObject>(),
                            (*member).name,
                        );
                        return ptr::null_mut();
                    }
                    let none = &raw mut crate::abi_types::Py_None;
                    crate::api::refcount::Py_INCREF(none);
                    none
                } else {
                    crate::api::refcount::Py_INCREF(v);
                    v
                }
            }
            PY_T_NONE => {
                let none = &raw mut crate::abi_types::Py_None;
                crate::api::refcount::Py_INCREF(none);
                none
            }
            _ => {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"bad member type in PyMember_GetOne".as_ptr(),
                );
                ptr::null_mut()
            }
        }
    }
}

/// `PyMember_SetOne` — write one struct member from a Python object. Faithful to
/// CPython `Python/structmember.c`, covering the mutable subset numpy uses (it
/// declares nearly all members `READONLY`). Read-only / audit-only members and
/// unsupported writes fail closed with an honest exception.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyMember_SetOne(
    addr: *mut c_char,
    member: *mut crate::abi_types::PyMemberDef,
    value: *mut PyObject,
) -> c_int {
    if addr.is_null() || member.is_null() {
        return -1;
    }
    unsafe {
        if (*member).flags & PY_READONLY != 0 {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_AttributeError)
                    .cast::<crate::abi_types::PyObject>(),
                c"readonly attribute".as_ptr(),
            );
            return -1;
        }
        let ty = (*member).type_;
        let field = addr.offset((*member).offset);
        // CPython Python/structmember.c delete (v == NULL) rules: only T_OBJECT
        // (unconditionally) and T_OBJECT_EX (when already set) may be deleted;
        // deleting a numeric/char member is a TypeError.
        if value.is_null() {
            if ty == PY_T_OBJECT_EX {
                if (*(field as *const *mut PyObject)).is_null() {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_AttributeError)
                            .cast::<crate::abi_types::PyObject>(),
                        (*member).name,
                    );
                    return -1;
                }
            } else if ty != PY_T_OBJECT {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"can't delete numeric/char attribute".as_ptr(),
                );
                return -1;
            }
        }
        // Helper: has an exception been raised by a converter?
        let err_set = || !crate::api::errors::PyErr_Occurred().is_null();
        // NOTE: CPython emits a non-fatal RuntimeWarning on out-of-range
        // truncation (the WARN macro); the stored (truncated) value and the
        // error/return contract are identical here — the warning is elided.
        match ty {
            PY_T_BOOL => {
                let is_true = std::ptr::eq(
                    value,
                    (&raw mut crate::abi_types::Py_True).cast::<PyObject>(),
                );
                let is_false = std::ptr::eq(
                    value,
                    (&raw mut crate::abi_types::Py_False).cast::<PyObject>(),
                );
                if !is_true && !is_false {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_TypeError)
                            .cast::<crate::abi_types::PyObject>(),
                        c"attribute value type must be bool".as_ptr(),
                    );
                    return -1;
                }
                *field.cast::<c_char>() = c_char::from(is_true);
                0
            }
            PY_T_BYTE => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                // CPython stores `(char)long_val`; the width is one byte on every target.
                *field.cast::<c_char>() = v as c_char;
                0
            }
            PY_T_UBYTE => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *field.cast::<u8>() = v as u8;
                0
            }
            PY_T_SHORT => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *(field as *mut i16) = v as i16;
                0
            }
            PY_T_USHORT => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *(field as *mut u16) = v as u16;
                0
            }
            PY_T_INT => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *(field as *mut i32) = v as i32;
                0
            }
            PY_T_UINT => {
                // CPython accepts negative ints for compatibility (falls back to
                // the signed converter after clearing the OverflowError).
                let mut u = crate::api::numbers::PyLong_AsUnsignedLong(value);
                if u == c_ulong::MAX && err_set() {
                    crate::api::errors::PyErr_Clear();
                    let s = crate::api::numbers::PyLong_AsLong(value);
                    if s == -1 && err_set() {
                        return -1;
                    }
                    u = s as c_ulong;
                }
                *(field as *mut u32) = u as u32;
                0
            }
            PY_T_LONG => {
                let v = crate::api::numbers::PyLong_AsLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *(field as *mut std::os::raw::c_long) = v;
                0
            }
            PY_T_ULONG => {
                let mut u = crate::api::numbers::PyLong_AsUnsignedLong(value);
                if u == c_ulong::MAX && err_set() {
                    crate::api::errors::PyErr_Clear();
                    let s = crate::api::numbers::PyLong_AsLong(value);
                    if s == -1 && err_set() {
                        return -1;
                    }
                    u = s as c_ulong;
                }
                *(field as *mut std::os::raw::c_ulong) = u;
                0
            }
            PY_T_PYSSIZET => {
                let v = crate::api::numbers::PyLong_AsSsize_t(value);
                if v == -1 && err_set() {
                    return -1;
                }
                *(field as *mut isize) = v;
                0
            }
            PY_T_LONGLONG => {
                let v = crate::api::numbers::PyLong_AsLongLong(value);
                if v == -1 && err_set() {
                    return -1;
                }
                // 8-byte member: `field` may be only 4-aligned on a C-minted
                // (wasm32, struct-align-4) object — see PyMember_GetOne's
                // read_unaligned for the same class (d461a6fea6). An aligned
                // write here would be UB (misaligned dereference).
                std::ptr::write_unaligned(field as *mut c_longlong, v);
                0
            }
            PY_T_ULONGLONG => {
                let mut u = crate::api::numbers::PyLong_AsUnsignedLongLong(value);
                if u == c_ulonglong::MAX && err_set() {
                    crate::api::errors::PyErr_Clear();
                    let s = crate::api::numbers::PyLong_AsLongLong(value);
                    if s == -1 && err_set() {
                        return -1;
                    }
                    u = s as c_ulonglong;
                }
                std::ptr::write_unaligned(field as *mut c_ulonglong, u);
                0
            }
            PY_T_FLOAT => {
                let v = crate::api::numbers::PyFloat_AsDouble(value);
                if v == -1.0 && err_set() {
                    return -1;
                }
                *(field as *mut f32) = v as f32;
                0
            }
            PY_T_DOUBLE => {
                let v = crate::api::numbers::PyFloat_AsDouble(value);
                if v == -1.0 && err_set() {
                    return -1;
                }
                // Same 8-byte alignment class as T_LONGLONG/T_ULONGLONG above.
                std::ptr::write_unaligned(field as *mut f64, v);
                0
            }
            PY_T_CHAR => {
                let mut len: Py_ssize_t = 0;
                let s = crate::api::strings::PyUnicode_AsUTF8AndSize(value, &raw mut len);
                if s.is_null() || len != 1 {
                    crate::api::errors::PyErr_BadArgument();
                    return -1;
                }
                *(field as *mut c_char) = *s;
                0
            }
            PY_T_STRING => {
                // T_STRING / T_STRING_INPLACE are readonly (CPython raises here).
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"readonly attribute".as_ptr(),
                );
                -1
            }
            PY_T_OBJECT | PY_T_OBJECT_EX => {
                let slot = field as *mut *mut PyObject;
                let old = *slot;
                if !value.is_null() {
                    crate::api::refcount::Py_INCREF(value);
                }
                *slot = value;
                if !old.is_null() {
                    crate::api::refcount::Py_DECREF(old);
                }
                0
            }
            _ => {
                // Unknown member type: SystemError "bad memberdescr type for %s".
                let name = if (*member).name.is_null() {
                    "?".to_string()
                } else {
                    std::ffi::CStr::from_ptr((*member).name)
                        .to_string_lossy()
                        .into_owned()
                };
                let msg = format!("bad memberdescr type for {name}");
                if let Ok(c) = std::ffi::CString::new(msg) {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_SystemError)
                            .cast::<crate::abi_types::PyObject>(),
                        c.as_ptr(),
                    );
                }
                -1
            }
        }
    }
}

/// Source-recompiled `Py_TYPE(op)` authority. Managed views report semantic
/// builtin identity while their physical carrier remains `MoltManaged_Type`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _Py_TYPE(op: *mut PyObject) -> *mut PyTypeObject {
    unsafe { crate::bridge::semantic_type(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Type(op: *mut PyObject) -> *mut PyObject {
    if op.is_null() {
        unsafe { crate::api::object::null_argument_error() };
        return ptr::null_mut();
    }
    let tp = unsafe { crate::bridge::semantic_type(op) };
    if tp.is_null() {
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
            && !crate::api::errors::transfer_runtime_pending_to_current()
        {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError)
                        .cast::<crate::abi_types::PyObject>(),
                    c"object has NULL type".as_ptr(),
                );
            }
        }
        return ptr::null_mut();
    }
    let type_obj = tp.cast::<PyObject>();
    unsafe { crate::api::refcount::Py_INCREF(type_obj) };
    type_obj
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_TypeCheck(op: *mut PyObject, tp: *mut PyTypeObject) -> c_int {
    if op.is_null() || tp.is_null() {
        return 0;
    }
    // CPython's `PyObject_TypeCheck` (Include/object.h) is
    //   `Py_IS_TYPE(ob, tp) || PyType_IsSubtype(Py_TYPE(ob), tp)`
    // — an EXACT-type match OR a subtype relationship. Molt previously answered
    // only the exact match, so any C extension that type-checks an instance
    // against a BASE type failed closed. numpy's `PyArray_DescrCheck(res)` is
    // `PyObject_TypeCheck(res, &PyArrayDescr_Type)`: a DType descriptor's
    // `Py_TYPE` is its concrete DType class (e.g. `StringDType`, whose
    // `tp_base == &PyArrayDescr_Type`), never `PyArrayDescr_Type` itself, so the
    // exact-only check rejected every genuine descriptor and stranded
    // `use_new_as_default` (dtypemeta.c) with "did not return a dtype instance".
    // Walk the subtype chain exactly as CPython does.
    let actual = unsafe { crate::bridge::semantic_type(op) };
    if actual.is_null() {
        return 0;
    }
    if std::ptr::eq(actual, tp) {
        return 1;
    }
    unsafe { PyType_IsSubtype(actual, tp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_IsInstance(inst: *mut PyObject, cls: *mut PyObject) -> c_int {
    unsafe {
        crate::api::object::classinfo_match(crate::hooks::ClassInfoOperation::Instance, inst, cls)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyCallable_Check(op: *mut PyObject) -> c_int {
    if op.is_null() {
        return 0;
    }
    if crate::bridge::resolve_pyobject(op).is_none() {
        return 0;
    }
    // A generic managed view has no authoritative C call slot. Its semantic
    // class projection is identity only; it may omit `tp_call` even when the
    // runtime class implements `__call__`. Use the same runtime predicate as
    // Python `callable()`. Concrete CFunction views and raw C objects retain
    // their physical slot protocol.
    if unsafe { crate::api::object::runtime_call_authority(op, false) } {
        if unsafe { crate::bridge::semantic_type(op) }.is_null() {
            return 0;
        }
        let Some(bits) = GLOBAL_BRIDGE.observed_handle_for_pyobj(op) else {
            if unsafe { crate::api::errors::PyErr_Occurred() }.is_null()
                && !crate::api::errors::transfer_runtime_pending_to_current()
            {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
            }
            return 0;
        };
        let result = unsafe { (crate::hooks::hooks_or_stubs().object_is_callable)(bits.bits()) };
        if crate::api::errors::transfer_runtime_pending_to_current() {
            return 0;
        }
        return c_int::from(result != 0);
    }
    let tp = unsafe { (*op).ob_type };
    if tp.is_null() {
        return 0;
    }
    if unsafe { (*tp).tp_call }.is_some() {
        return 1;
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Hash(op: *mut PyObject) -> isize {
    if op.is_null() {
        return -1;
    }
    // Molt-native (bridge-managed) objects hash through the runtime hash
    // authority over their handle bits (hash(int) == int, etc.), not tp_hash.
    let Some(observed) = crate::bridge::observe_pyobject(op) else {
        // A failed managed commit is an error, never permission to dispatch a
        // foreign hash slot or replace the original failure with TypeError.
        return -1;
    };
    if std::ptr::eq(
        unsafe { (*op).ob_type },
        &raw const crate::abi_types::PyComplex_Type,
    ) {
        return unsafe { complex_hash_from_cval(op) };
    }
    if let crate::bridge::ResolvedPyObject::ManagedMolt(value) = observed {
        return crate::bridge::molt_hash_from_bits(value.bits());
    }
    // Foreign object: dispatch tp_hash.
    let tp = unsafe { (*op).ob_type };
    if !tp.is_null()
        && let Some(hash_fn) = unsafe { (*tp).tp_hash }
    {
        return unsafe { hash_fn(op) };
    }
    // CPython lazily readies an unready type, then retries its inherited hash
    // slot. Readiness failure keeps its own exception.
    if !tp.is_null() && unsafe { (*tp).tp_flags } & Py_TPFLAGS_READY == 0 {
        if unsafe { PyType_Ready(tp) } < 0 {
            return -1;
        }
        if let Some(hash_fn) = unsafe { (*tp).tp_hash } {
            return unsafe { hash_fn(op) };
        }
    }
    unsafe { hash_not_implemented(op) }
}

// ─── PyType subtype / flags / name ────────────────────────────────────────

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_IsSubtype(a: *mut PyTypeObject, b: *mut PyTypeObject) -> c_int {
    if a.is_null() || b.is_null() {
        return 0;
    }
    let Some(a_identity) = crate::bridge::resolve_pyobject(a.cast()) else {
        return 0;
    };
    let b_identity = if std::ptr::eq(a, b) {
        a_identity
    } else {
        let Some(identity) = crate::bridge::resolve_pyobject(b.cast()) else {
            return 0;
        };
        identity
    };
    if let (
        crate::bridge::ResolvedPyObject::ManagedMolt(a_bits),
        crate::bridge::ResolvedPyObject::ManagedMolt(b_bits),
    ) = (a_identity, b_identity)
    {
        let hooks = crate::hooks::hooks_or_stubs();
        if unsafe { (hooks.classify_heap)(a_bits.bits()) }
            == crate::abi_types::MoltTypeTag::Type as u8
            && unsafe { (hooks.classify_heap)(b_bits.bits()) }
                == crate::abi_types::MoltTypeTag::Type as u8
        {
            return unsafe { (hooks.type_is_subtype)(a_bits.bits(), b_bits.bits()) };
        }
    }
    // CPython Objects/typeobject.c: when `a` has a materialized tp_mro, walk the
    // full MRO tuple (this is what makes MULTIPLE inheritance resolve — numpy's
    // dual-inherit scalar types, e.g. `np.int_` from both `signedinteger` and
    // `int`, are only reachable via the MRO, never the tp_base primary chain).
    let mro = unsafe { (*a).tp_mro };
    if !mro.is_null() {
        let n = unsafe { crate::api::sequences::PyTuple_Size(mro) };
        let mut i: Py_ssize_t = 0;
        while i < n {
            let entry = unsafe { crate::api::sequences::PyTuple_GetItem(mro, i) };
            if std::ptr::eq(entry.cast::<PyTypeObject>(), b) {
                return 1;
            }
            if let Some(secondary) = crate::abi_types::exc_singleton_secondary_parent(entry) {
                let secondary = secondary.cast::<PyTypeObject>();
                if std::ptr::eq(secondary, b) || unsafe { PyType_IsSubtype(secondary, b) } != 0 {
                    return 1;
                }
            }
            i += 1;
        }
        return 0;
    }
    // `a` is not completely initialized (no tp_mro yet): follow the tp_base
    // primary chain, and — matching CPython's type_is_subtype_base_chain — treat
    // every fully-walked type as a subtype of `object` at the chain end.
    let mut cursor = a;
    while !cursor.is_null() {
        if std::ptr::eq(cursor, b) {
            return 1;
        }
        if let Some(secondary) =
            crate::abi_types::exc_singleton_secondary_parent(cursor.cast::<PyObject>())
        {
            let secondary = secondary.cast::<PyTypeObject>();
            if std::ptr::eq(secondary, b) || unsafe { PyType_IsSubtype(secondary, b) } != 0 {
                return 1;
            }
        }
        cursor = unsafe { (*cursor).tp_base };
    }
    std::ptr::eq(b, &raw mut crate::abi_types::PyBaseObject_Type) as c_int
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetFlags(tp: *mut PyTypeObject) -> std::os::raw::c_ulong {
    if tp.is_null() {
        return 0;
    }
    unsafe { (*tp).tp_flags }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetName(tp: *mut PyTypeObject) -> *mut PyObject {
    if tp.is_null() {
        return ptr::null_mut();
    }
    if let Some(value) = GLOBAL_BRIDGE
        .observed_handle_for_pyobj(tp.cast())
        .filter(|value| unsafe {
            (crate::hooks::hooks_or_stubs().classify_heap)(value.bits())
                == crate::abi_types::MoltTypeTag::Type as u8
        })
    {
        return unsafe { managed_type_name(value.bits(), crate::hooks::TypeMetadataField::Name) };
    }
    if let Some(heap) = heap_type_storage(tp) {
        return unsafe { crate::api::object::Py_XNewRef((*heap).ht_name) };
    }
    let name_ptr = unsafe { (*tp).tp_name };
    if name_ptr.is_null() {
        return ptr::null_mut();
    }
    // Native static types derive the short name from their physical tp_name.
    let bytes = unsafe { std::ffi::CStr::from_ptr(name_ptr) }.to_bytes();
    let short = match bytes.iter().rposition(|&b| b == b'.') {
        Some(dot) => &bytes[dot + 1..],
        None => bytes,
    };
    let decoded = String::from_utf8_lossy(short);
    unsafe {
        crate::api::strings::unicode_from_python_text(
            crate::api::strings::PythonStringBytes::from_utf8(&decoded),
        )
    }
}

/// Return a new reference to the canonical type dictionary (CPython 3.12+).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetDict(tp: *mut PyTypeObject) -> *mut PyObject {
    unsafe { crate::api::object::Py_XNewRef(type_dict_borrowed(tp)) }
}

/// Internal lookup borrows the same owner; attribute lookup would instead
/// return a mapping proxy. Callers retaining it across callbacks acquire a pin.
pub(crate) unsafe fn type_dict_borrowed(tp: *mut PyTypeObject) -> *mut PyObject {
    if tp.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    unsafe {
        if ((*tp).tp_dict.is_null() || (*tp).tp_flags & Py_TPFLAGS_READY == 0)
            && PyType_Ready(tp) < 0
        {
            return ptr::null_mut();
        }
        (*tp).tp_dict
    }
}

unsafe fn managed_type_name(bits: u64, field: crate::hooks::TypeMetadataField) -> *mut PyObject {
    let result = unsafe { (crate::hooks::hooks_or_stubs().type_metadata)(bits, field) };
    unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(result) }
}

/// CPython _PyType_Name semantics differ from PyType_GetName for a dotted heap
/// __name__: repr uses only the final component. Read current managed metadata;
/// native C types retain their physical tp_name authority.
pub(crate) unsafe fn type_short_name_bytes(tp: *mut PyTypeObject) -> Option<Vec<u8>> {
    if tp.is_null() {
        return None;
    }
    let bytes = if GLOBAL_BRIDGE.observed_handle_for_pyobj(tp.cast()).is_some() {
        let name = unsafe { PyType_GetName(tp) };
        if name.is_null() {
            return None;
        }
        let bytes = unsafe { crate::api::strings::unicode_bytes(name) }.map(<[u8]>::to_vec);
        unsafe { crate::api::errors::release_preserving_error(&[name]) };
        bytes?
    } else {
        let name = unsafe { (*tp).tp_name };
        if name.is_null() {
            return None;
        }
        String::from_utf8_lossy(unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes())
            .into_owned()
            .into_bytes()
    };
    let start = bytes
        .iter()
        .rposition(|byte| *byte == b'.')
        .map_or(0, |index| index + 1);
    Some(bytes[start..].to_vec())
}

/// Metatype attribute lookup shares the public name observers. Managed types
/// read callback-free structural metadata; native heap types use distinct
/// ht_name/ht_qualname fields; static types derive their short tp_name. All
/// other attributes retain normal metatype-descriptor and type-MRO precedence.
unsafe extern "C" fn type_getattro(o: *mut PyObject, name: *mut PyObject) -> *mut PyObject {
    use crate::api::{descriptor, refcount::OwnedPyObject};
    if o.is_null() || name.is_null() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let Some(_type_owner) = (unsafe { OwnedPyObject::try_from_borrowed(o) }) else {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    };
    let _name_owner = unsafe { OwnedPyObject::from_borrowed(name) };
    if descriptors::pending() {
        return ptr::null_mut();
    }
    let tp = o.cast::<PyTypeObject>();
    let metatype = unsafe { (*o).ob_type };
    let _metatype_owner = unsafe { OwnedPyObject::from_borrowed(metatype.cast()) };
    let meta_owner = unsafe { OwnedPyObject::from_borrowed(_PyType_Lookup(metatype, name)) };
    if descriptors::pending() {
        return ptr::null_mut();
    }
    let meta_attribute = meta_owner.as_ptr();
    let is_data = match unsafe { descriptor::is_data(meta_attribute) } {
        Ok(value) => value,
        Err(crate::ErrorIndicatorSet) => return ptr::null_mut(),
    };
    if is_data && let Some(result) = unsafe { descriptor::get(meta_attribute, o, metatype.cast()) }
    {
        return result;
    }

    let owner = unsafe { OwnedPyObject::from_borrowed(_PyType_Lookup(tp, name)) };
    if descriptors::pending() {
        return ptr::null_mut();
    }
    let attribute = owner.as_ptr();
    if !attribute.is_null() {
        if let Some(result) = unsafe { descriptor::get(attribute, ptr::null_mut(), o) } {
            return result;
        }
        unsafe { crate::api::refcount::Py_INCREF(attribute) };
        return attribute;
    }

    if !meta_attribute.is_null() {
        if let Some(result) = unsafe { descriptor::get(meta_attribute, o, metatype.cast()) } {
            return result;
        }
        unsafe { crate::api::refcount::Py_INCREF(meta_attribute) };
        return meta_attribute;
    }

    unsafe { crate::api::object::PyObject_GenericGetAttr(o, name) }
}

/// Install `type_getattro` on `PyType_Type` so metaclasses inherit it. Called
/// by the process ABI bootstrap after the static type table is zero-initialized.
///
/// # Safety
/// Must be called during single-threaded ABI initialization.
pub unsafe fn init_type_getattro() {
    unsafe {
        crate::abi_types::PyType_Type.tp_getattro = Some(type_getattro);
        crate::abi_types::PyType_Type.tp_setattro = Some(root_metadata::type_setattro);
    }
}

/// Ensure the *metatype* of a just-readied type `tp` carries a `tp_getattro`.
///
/// In CPython every metaclass inherits `type.__getattribute__` (our
/// `type_getattro`) from `type`; a static extension's metaclass (numpy's
/// `_DTypeMeta`) should get it when `PyType_Ready` runs. But in the
/// split-runtime the extension's `&PyType_Type` can retarget to a copy of
/// `type` that never received our `type_getattro`, so `inherit_slots` copies a
/// null slot and the metaclass is left with no getattro — making
/// `DType.__name__` (numpy's `numpy.dtypes._add_dtype_helper`) fail to resolve.
/// `tp`'s metatype is the object that answers attribute access for `tp` and
/// every sibling instance of that metaclass, so installing our `type_getattro`
/// here (only when neither getter is declared) makes
/// `Type.__name__` / `__qualname__` resolve for every type of that metaclass,
/// from Molt (via foreign-object custody) and from C alike.
///
/// # Safety
/// `tp` must be a readied `PyTypeObject` (its `ob_type` is set).
pub(crate) unsafe fn install_metatype_getattro(tp: *mut PyTypeObject) {
    if tp.is_null() {
        return;
    }
    let metatype = unsafe { (*tp).ob_base.ob_base.ob_type };
    if metatype.is_null() || std::ptr::eq(metatype, tp) {
        return;
    }
    if unsafe { (*metatype).tp_getattr }.is_none() && unsafe { (*metatype).tp_getattro }.is_none() {
        unsafe { (*metatype).tp_getattro = Some(type_getattro) };
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_GetQualName(tp: *mut PyTypeObject) -> *mut PyObject {
    if tp.is_null() {
        return ptr::null_mut();
    }
    if let Some(value) = GLOBAL_BRIDGE
        .observed_handle_for_pyobj(tp.cast())
        .filter(|value| unsafe {
            (crate::hooks::hooks_or_stubs().classify_heap)(value.bits())
                == crate::abi_types::MoltTypeTag::Type as u8
        })
    {
        return unsafe {
            managed_type_name(value.bits(), crate::hooks::TypeMetadataField::QualName)
        };
    }
    if let Some(heap) = heap_type_storage(tp) {
        return unsafe { crate::api::object::Py_XNewRef((*heap).ht_qualname) };
    }
    unsafe { PyType_GetName(tp) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyType_HasFeature(
    tp: *mut PyTypeObject,
    feature: std::os::raw::c_ulong,
) -> c_int {
    if tp.is_null() {
        return 0;
    }
    (unsafe { (*tp).tp_flags } & feature != 0) as c_int
}

/// Best-effort semantic type name of a live `PyObject*` for diagnostics
/// (mirrors CPython's `Py_TYPE(v)->tp_name`, defaulting to `object`).
unsafe fn object_type_name(op: *mut PyObject) -> String {
    unsafe { object_type_name_with_precision(op, usize::MAX) }
}

/// CPython's %.Ns truncates raw tp_name bytes before UTF-8 replacement decoding.
pub unsafe fn object_type_name_with_precision(op: *mut PyObject, precision: usize) -> String {
    if op.is_null() {
        return "object".to_string();
    }
    let tp = unsafe { crate::bridge::semantic_type(op) };
    if tp.is_null() {
        return "object".to_string();
    }
    let name = unsafe { (*tp).tp_name };
    if name.is_null() {
        return "object".to_string();
    }
    let bytes = if precision == usize::MAX {
        unsafe { std::ffi::CStr::from_ptr(name) }.to_bytes()
    } else {
        let mut length = 0;
        while length < precision && unsafe { *name.add(length) } != 0 {
            length += 1;
        }
        unsafe { std::slice::from_raw_parts(name.cast::<u8>(), length) }
    };
    String::from_utf8_lossy(bytes).into_owned()
}

/// Validate that a `tp_str`/`tp_repr` slot returned an actual `str`, mirroring
/// CPython's `__str__/__repr__ returned non-string (type %.200s)` guard.
/// Consumes `res` on the error path (Py_DECREF) and returns NULL with a
/// pending `TypeError`; otherwise returns `res` unchanged.
unsafe fn check_stringifier_result(res: *mut PyObject, dunder: &str) -> *mut PyObject {
    if res.is_null() {
        // The slot already set the exception — propagate as-is.
        return ptr::null_mut();
    }
    if unsafe { crate::api::strings::PyUnicode_Check(res) } == 0 {
        let name = unsafe { object_type_name_with_precision(res, 200) };
        let msg = format!("{dunder} returned non-string (type {name})");
        if let Ok(cmsg) = std::ffi::CString::new(msg) {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_TypeError)
                        .cast::<crate::abi_types::PyObject>(),
                    cmsg.as_ptr(),
                );
            }
        }
        unsafe { crate::api::errors::release_preserving_error(&[res]) };
        return ptr::null_mut();
    }
    res
}

/// Project the owned result from the canonical runtime str/repr protocol.
/// This does not copy or format bytes and preserves the exact pending error.
unsafe fn native_stringify(bits: u64, want_repr: bool) -> *mut PyObject {
    let hooks = crate::hooks::hooks_or_stubs();
    let result = if want_repr {
        unsafe { (hooks.object_repr)(bits) }
    } else {
        unsafe { (hooks.object_str)(bits) }
    };
    let result = unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(result) };
    unsafe { check_stringifier_result(result, if want_repr { "__repr__" } else { "__str__" }) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_native_repr(op: *mut PyObject) -> *mut PyObject {
    let native = crate::bridge::GLOBAL_BRIDGE.observed_handle_for_pyobj(op);
    match native {
        Some(value) => unsafe { native_stringify(value.bits(), true) },
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_native_str(op: *mut PyObject) -> *mut PyObject {
    let native = crate::bridge::GLOBAL_BRIDGE.observed_handle_for_pyobj(op);
    match native {
        Some(value) => unsafe { native_stringify(value.bits(), false) },
        None => ptr::null_mut(),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Repr(op: *mut PyObject) -> *mut PyObject {
    // CPython Objects/object.c PyObject_Repr: NULL -> "<NULL>".
    if op.is_null() {
        return unsafe { crate::api::strings::PyUnicode_FromString(c"<NULL>".as_ptr()) };
    }
    match crate::bridge::observe_pyobject(op) {
        Some(crate::bridge::ResolvedPyObject::ManagedMolt(value)) => {
            return unsafe { native_stringify(value.bits(), true) };
        }
        None => return ptr::null_mut(),
        Some(crate::bridge::ResolvedPyObject::Foreign) => {}
    }
    let tp = unsafe { (*op).ob_type };
    if !tp.is_null()
        && let Some(reprfunc) = unsafe { (*tp).tp_repr }
    {
        if unsafe {
            crate::api::memory::Py_EnterRecursiveCall(
                c" while getting the repr of an object".as_ptr(),
            )
        } != 0
        {
            return ptr::null_mut();
        }
        let res = unsafe { reprfunc(op) };
        unsafe { crate::api::memory::Py_LeaveRecursiveCall() };
        return unsafe { check_stringifier_result(res, "__repr__") };
    }
    unsafe { object_repr(op) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_Str(op: *mut PyObject) -> *mut PyObject {
    // CPython Objects/object.c PyObject_Str: NULL -> "<NULL>".
    if op.is_null() {
        return unsafe { crate::api::strings::PyUnicode_FromString(c"<NULL>".as_ptr()) };
    }
    // Managed string identity and every subclass override belong to the same
    // runtime authority. A semantic type projection is not a native slot table.
    match crate::bridge::observe_pyobject(op) {
        Some(crate::bridge::ResolvedPyObject::ManagedMolt(value)) => {
            return unsafe { native_stringify(value.bits(), false) };
        }
        None => return ptr::null_mut(),
        Some(crate::bridge::ResolvedPyObject::Foreign) => {}
    }
    let tp = unsafe { (*op).ob_type };
    if tp == &raw mut crate::abi_types::PyUnicode_Type {
        unsafe { crate::api::refcount::Py_INCREF(op) };
        return op;
    }
    if !tp.is_null()
        && let Some(strfunc) = unsafe { (*tp).tp_str }
    {
        if unsafe {
            crate::api::memory::Py_EnterRecursiveCall(
                c" while getting the str of an object".as_ptr(),
            )
        } != 0
        {
            return ptr::null_mut();
        }
        let res = unsafe { strfunc(op) };
        unsafe { crate::api::memory::Py_LeaveRecursiveCall() };
        return unsafe { check_stringifier_result(res, "__str__") };
    }
    unsafe { PyObject_Repr(op) }
}

// Comparison opcodes (CPython Include/object.h): Py_LT..Py_GE = 0..5.
const CMP_LT: c_int = RichCompareOp::Lt as c_int;
const CMP_LE: c_int = RichCompareOp::Le as c_int;
const CMP_EQ: c_int = RichCompareOp::Eq as c_int;
const CMP_NE: c_int = RichCompareOp::Ne as c_int;
const CMP_GT: c_int = RichCompareOp::Gt as c_int;
const CMP_GE: c_int = RichCompareOp::Ge as c_int;

/// `_Py_SwappedOp[op]` — the reflected comparison operator.
#[inline]
fn swapped_op(op: c_int) -> c_int {
    RichCompareOp::from_i32(op).map_or(op, |op| op.reversed() as c_int)
}

#[inline]
fn cmp_opstring(op: c_int) -> &'static str {
    match op {
        CMP_LT => "<",
        CMP_LE => "<=",
        CMP_EQ => "==",
        CMP_NE => "!=",
        CMP_GT => ">",
        CMP_GE => ">=",
        _ => "?",
    }
}

#[inline]
fn is_not_implemented(res: *mut PyObject) -> bool {
    std::ptr::eq(res, &raw mut crate::abi_types::Py_NotImplementedSentinel)
}

/// Call a `tp_richcompare` slot, returning `Some(result)` when the slot exists
/// (NULL result = pending error, NotImplemented = "not handled") or `None` when
/// the type carries no slot.
unsafe fn try_slot_richcompare(
    tp: *mut PyTypeObject,
    a: *mut PyObject,
    b: *mut PyObject,
    op: c_int,
) -> Option<*mut PyObject> {
    // Both semantic classes are admitted once by do_richcompare before any
    // callback. Re-observing carriers here would create a second dispatch
    // authority and could turn a failed lookup into an absent slot.
    let f = unsafe { (*tp).tp_richcompare }?;
    Some(unsafe { f(a, b, op) })
}

#[inline]
fn cmp_bool_result(b: bool) -> *mut PyObject {
    let res = if b {
        (&raw mut crate::abi_types::Py_True).cast::<PyObject>()
    } else {
        (&raw mut crate::abi_types::Py_False).cast::<PyObject>()
    };
    unsafe { crate::api::refcount::Py_INCREF(res) };
    res
}

/// Return the declaring slot's owned NotImplemented result.
#[inline]
unsafe fn richcmp_not_implemented() -> *mut PyObject {
    let result = &raw mut crate::abi_types::Py_NotImplementedSentinel;
    unsafe { crate::api::refcount::Py_INCREF(result) };
    result
}

/// Invoke a canonical builtin declaring class without generic redispatch.
/// The caller has admitted physical operand families and owns their values.
pub(crate) unsafe fn declaring_richcompare(
    declaring_type: *mut PyTypeObject,
    left: u64,
    right: u64,
    op: c_int,
) -> *mut PyObject {
    if RichCompareOp::from_i32(op).is_none() {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    }
    let Some(owner) = crate::bridge::resolved_molt_handle(declaring_type.cast()) else {
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe {
                crate::api::errors::PyErr_SetString(
                    (&raw mut crate::abi_types::PyExc_SystemError).cast::<PyObject>(),
                    c"builtin comparison owner has no runtime class identity".as_ptr(),
                )
            };
        }
        return ptr::null_mut();
    };
    let result = unsafe {
        (crate::hooks::hooks_or_stubs().object_richcompare_builtin)(owner.bits(), op, left, right)
    };
    unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(result) }
}

unsafe fn numeric_declaring_richcompare(
    declaring_type: *mut PyTypeObject,
    left: *mut PyObject,
    right: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    let Some(left) = (unsafe { crate::api::numbers::numeric_comparison_value(left) }) else {
        return ptr::null_mut();
    };
    let Some(right) = (unsafe { crate::api::numbers::numeric_comparison_value(right) }) else {
        return ptr::null_mut();
    };
    unsafe { declaring_richcompare(declaring_type, left.bits(), right.bits(), op) }
}

/// A declaring byte comparison borrows storage only; the shared comparator
/// performs no callbacks. Raw native bytes retain their truthful C prefix.
unsafe fn comparison_bytes<'a>(
    object: *mut PyObject,
    kind: crate::abi_types::MoltTypeTag,
) -> Option<&'a [u8]> {
    if let crate::bridge::ResolvedPyObject::ManagedMolt(handle) =
        crate::bridge::observe_pyobject(object)?
    {
        let hooks = crate::hooks::hooks_or_stubs();
        let mut len = 0;
        let data = unsafe {
            if kind == crate::abi_types::MoltTypeTag::Str {
                (hooks.str_data)(handle.bits(), &raw mut len)
            } else {
                (hooks.bytes_data)(handle.bits(), &raw mut len)
            }
        };
        if data.is_null() {
            if !crate::api::errors::transfer_runtime_pending_to_current() {
                unsafe { crate::api::errors::PyErr_BadInternalCall() };
            }
            return None;
        }
        return Some(unsafe { std::slice::from_raw_parts(data, len) });
    }
    if kind == crate::abi_types::MoltTypeTag::Bytes {
        let object = object.cast::<crate::abi_types::PyBytesObject>();
        let len = unsafe { (*object).ob_base.ob_size };
        if len < 0 {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
            return None;
        }
        let data = unsafe { (&raw const (*object).ob_sval).cast::<u8>() };
        return Some(unsafe { std::slice::from_raw_parts(data, len as usize) });
    }
    None
}

unsafe fn bytes_declaring_richcompare(
    left: *mut PyObject,
    right: *mut PyObject,
    op: c_int,
    kind: crate::abi_types::MoltTypeTag,
) -> *mut PyObject {
    let Some(op) = RichCompareOp::from_i32(op) else {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    };
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    let Some(left) = (unsafe { comparison_bytes(left, kind) }) else {
        return if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe { richcmp_not_implemented() }
        } else {
            ptr::null_mut()
        };
    };
    let Some(right) = (unsafe { comparison_bytes(right, kind) }) else {
        return if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe { richcmp_not_implemented() }
        } else {
            ptr::null_mut()
        };
    };
    cmp_bool_result(op.test(molt_lang_obj_model::byte_compare::compare_bytes(
        left, right,
    )))
}

/// int's declaring slot accepts only int/bool storage; float reflection owns
/// mixed int/float comparison. Numeric values are compared by the runtime.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_long_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    if unsafe { crate::api::numbers::PyLong_Check(v) } == 0
        || unsafe { crate::api::numbers::PyLong_Check(w) } == 0
    {
        return unsafe { richcmp_not_implemented() };
    }
    unsafe { numeric_declaring_richcompare(&raw mut crate::abi_types::PyLong_Type, v, w, op) }
}

/// float's declaring slot accepts float receivers and float/int peers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_float_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    if unsafe { crate::api::numbers::PyFloat_Check(v) } == 0
        || (unsafe { crate::api::numbers::PyFloat_Check(w) } == 0
            && unsafe { crate::api::numbers::PyLong_Check(w) } == 0)
    {
        return unsafe { richcmp_not_implemented() };
    }
    unsafe { numeric_declaring_richcompare(&raw mut crate::abi_types::PyFloat_Type, v, w, op) }
}

/// str's declaring slot admits only string storage, then shares the runtime's
/// code-point-preserving byte order without invoking outer subclass overrides.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_str_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    if unsafe { crate::api::strings::PyUnicode_Check(v) } == 0
        || unsafe { crate::api::strings::PyUnicode_Check(w) } == 0
    {
        return unsafe { richcmp_not_implemented() };
    }
    unsafe { bytes_declaring_richcompare(v, w, op, crate::abi_types::MoltTypeTag::Str) }
}

/// bytes's declaring slot admits only bytes storage. Bytearray reflection owns
/// a mixed bytes/bytearray operation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_bytes_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    if unsafe { crate::api::strings::PyBytes_Check(v) } == 0
        || unsafe { crate::api::strings::PyBytes_Check(w) } == 0
    {
        return unsafe { richcmp_not_implemented() };
    }
    unsafe { bytes_declaring_richcompare(v, w, op, crate::abi_types::MoltTypeTag::Bytes) }
}

/// A native bytes exporter uses the same storage reader as its declaring
/// comparison slot and the canonical public buffer ownership transaction.
pub unsafe extern "C" fn molt_bytes_getbuffer(
    object: *mut PyObject,
    view: *mut crate::abi_types::Py_buffer,
    flags: c_int,
) -> c_int {
    if unsafe { crate::api::strings::PyBytes_Check(object) } == 0 {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return -1;
    }
    let Some(bytes) = (unsafe { comparison_bytes(object, crate::abi_types::MoltTypeTag::Bytes) })
    else {
        return -1;
    };
    unsafe {
        crate::api::buffer::PyBuffer_FillInfo(
            view,
            object,
            bytes.as_ptr().cast_mut().cast(),
            bytes.len() as Py_ssize_t,
            1,
            flags,
        )
    }
}

/// Bytearray owns mixed bytes-like comparison. Hold the receiver's export
/// before invoking the peer exporter, preserving the protocol's resize guard.
pub unsafe extern "C" fn molt_bytearray_richcompare(
    left: *mut PyObject,
    right: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    let Some(op) = RichCompareOp::from_i32(op) else {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        return ptr::null_mut();
    };
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    let Some(resolved) = crate::bridge::observe_pyobject(left) else {
        return if left.is_null() {
            unsafe { richcmp_not_implemented() }
        } else {
            ptr::null_mut()
        };
    };
    let managed = matches!(resolved, crate::bridge::ResolvedPyObject::ManagedMolt(_));
    let admitted = if managed {
        let ty = unsafe { crate::bridge::semantic_type(left) };
        if ty.is_null() {
            return ptr::null_mut();
        }
        (unsafe { PyType_IsSubtype(ty, &raw mut crate::abi_types::PyByteArray_Type) }) != 0
    } else {
        (unsafe { crate::api::strings::PyByteArray_Check(left) }) != 0
    };
    if !admitted {
        return unsafe { richcmp_not_implemented() };
    }
    if unsafe { crate::api::buffer::PyObject_CheckBuffer(left) } == 0
        || unsafe { crate::api::buffer::PyObject_CheckBuffer(right) } == 0
    {
        return unsafe { richcmp_not_implemented() };
    }
    let mut receiver: crate::abi_types::Py_buffer = unsafe { std::mem::zeroed() };
    if unsafe {
        crate::api::buffer::PyObject_GetBuffer(
            left,
            &raw mut receiver,
            crate::abi_types::PyBUF_SIMPLE,
        )
    } != 0
    {
        unsafe { crate::api::errors::PyErr_Clear() };
        return unsafe { richcmp_not_implemented() };
    }
    let mut peer: crate::abi_types::Py_buffer = unsafe { std::mem::zeroed() };
    if unsafe {
        crate::api::buffer::PyObject_GetBuffer(right, &raw mut peer, crate::abi_types::PyBUF_SIMPLE)
    } != 0
    {
        // bytearray declines either unavailable simple export. Retire the
        // acquisition error before releasing the already-held receiver.
        unsafe {
            crate::api::errors::PyErr_Clear();
            crate::api::buffer::PyBuffer_Release(&raw mut receiver);
        }
        return unsafe { richcmp_not_implemented() };
    }
    let data = receiver.buf.cast::<u8>();
    let len = receiver.len;
    let valid = len >= 0
        && peer.len >= 0
        && (len == 0 || !data.is_null())
        && (peer.len == 0 || !peer.buf.is_null());
    let result = if valid {
        let left = if len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(data, len as usize) }
        };
        let right = if peer.len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(peer.buf.cast::<u8>(), peer.len as usize) }
        };
        cmp_bool_result(op.test(molt_lang_obj_model::byte_compare::compare_bytes(
            left, right,
        )))
    } else {
        unsafe { crate::api::errors::PyErr_BadInternalCall() };
        ptr::null_mut()
    };
    crate::api::errors::with_preserved_error(|| unsafe {
        crate::api::buffer::PyBuffer_Release(&raw mut receiver);
        crate::api::buffer::PyBuffer_Release(&raw mut peer);
    });
    result
}

/// Complex declares equality only, with complex/float/int peers. Exact mixed
/// numeric comparison and NaN behavior belong to the runtime family kernel.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_complex_richcompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    if (op != CMP_EQ && op != CMP_NE)
        || unsafe { crate::api::numbers::PyComplex_Check(v) } == 0
        || (unsafe { crate::api::numbers::PyComplex_Check(w) } == 0
            && unsafe { crate::api::numbers::PyFloat_Check(w) } == 0
            && unsafe { crate::api::numbers::PyLong_Check(w) } == 0)
    {
        return unsafe { richcmp_not_implemented() };
    }
    unsafe { numeric_declaring_richcompare(&raw mut crate::abi_types::PyComplex_Type, v, w, op) }
}
// ─── Builtin value-type `tp_hash` slot (CLASS1-SLOTS) ────────────────────────

/// Read `ob_fval` off a foreign object whose type is `float`-layout-compatible
/// (CPython `PyFloatObject = {PyObject_HEAD; double}`; `np.float64` shares it —
/// which is exactly why numpy DUAL_INHERITs `PyFloat_Type`'s slots onto its
/// Double scalar). Unaligned: a statically C-minted object may be pointer- (4-byte
/// on wasm32) aligned while the `double` wants 8 — same UB class the complex
/// readers guard.
#[inline]
unsafe fn read_foreign_ob_fval(op: *mut PyObject) -> f64 {
    let field = unsafe { (op as *const u8).add(std::mem::size_of::<PyObject>()) as *const f64 };
    unsafe { std::ptr::read_unaligned(field) }
}

/// CPython `Objects/complexobject.c` `complex_hash` (:405):
/// `hash(z) = hash(z.real) + _PyHASH_IMAG * hash(z.imag)` in wrapping unsigned
/// arithmetic, `-1 → -2`. `_PyHASH_IMAG = 1000003` (`Include/pyhash.h`). Each part
/// hashes through the runtime float-hash authority (`molt_hash_from_bits`), so
/// when `imag == 0` the result is exactly `hash(float real)` — preserving the
/// cross-type invariant `hash(x+0j) == hash(x)` within molt.
#[inline]
unsafe fn complex_hash_from_cval(op: *mut PyObject) -> isize {
    let cval = unsafe { crate::api::numbers::PyComplex_AsCComplex(op) };
    let part_hash = |d: f64| -> isize {
        crate::bridge::molt_hash_from_bits(molt_lang_obj_model::MoltObject::from_float(d).bits())
    };
    molt_lang_obj_model::hash_policy::combine_complex_hashes(
        part_hash(cval.real) as i64,
        part_hash(cval.imag) as i64,
    ) as isize
}

/// Set the CPython `unhashable type: '<name>'` TypeError and return the `-1`
/// error sentinel — the `PyObject_HashNotImplemented` contract. Used when a
/// foreign object reaches a copied builtin hash slot but has no molt-native
/// handle and no known-compatible C layout (never fabricate an identity hash).
#[inline]
unsafe fn hash_not_implemented(op: *mut PyObject) -> isize {
    let name = unsafe { object_type_name_with_precision(op, 200) };
    let msg = format!("unhashable type: '{name}'");
    if let Ok(cmsg) = std::ffi::CString::new(msg) {
        unsafe {
            crate::api::errors::PyErr_SetString(
                (&raw mut crate::abi_types::PyExc_TypeError).cast::<crate::abi_types::PyObject>(),
                cmsg.as_ptr(),
            );
        }
    }
    -1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_HashNotImplemented(op: *mut PyObject) -> isize {
    if crate::bridge::resolve_pyobject(op).is_none() {
        return -1;
    }
    unsafe { hash_not_implemented(op) }
}

/// Generic `tp_hash` slot for the builtin value types (int/bool/float/str/bytes/
/// complex). numpy DUAL_INHERIT copies these off molt's statics; a NULL slot
/// leaves numpy's scalars unhashable and breaks init. Routes a molt-native value
/// through `bridge::molt_hash_from_bits` (the same authority `PyObject_Hash`
/// uses — `hash(int)==int`, consistent, no drift); a foreign `complex`/`float`
/// through its layout-compatible C struct; else honest `unhashable` TypeError.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_generic_hash(op: *mut PyObject) -> isize {
    if op.is_null() {
        return -1;
    }
    if std::ptr::eq(
        unsafe { (*op).ob_type },
        &raw const crate::abi_types::PyComplex_Type,
    ) {
        return unsafe { complex_hash_from_cval(op) };
    }
    // Molt-native value. Decode-safe converter excludes a raw-registered foreign
    // object's `0xA11C` identity anchor (Class-2 mis-decode), so it is NEVER
    // hashed as a garbage float. Resolve then drop the bridge lock before hashing.
    let native = crate::bridge::GLOBAL_BRIDGE.observed_handle_for_pyobj(op);
    if let Some(bits) = native {
        return crate::bridge::molt_hash_from_bits(bits.bits());
    }
    // Foreign object carrying a copied builtin hash slot. complex and float have
    // CPython-defined layouts numpy's CDouble/Double scalars share.
    if unsafe { crate::api::numbers::PyComplex_Check(op) } != 0 {
        return unsafe { complex_hash_from_cval(op) };
    }
    if unsafe { crate::api::numbers::PyFloat_Check(op) } != 0 {
        let d = unsafe { read_foreign_ob_fval(op) };
        return crate::bridge::molt_hash_from_bits(
            molt_lang_obj_model::MoltObject::from_float(d).bits(),
        );
    }
    // A TYPE object (class) reaching this value-type hash slot — a numpy
    // metatype inherits/copies molt_generic_hash via DUAL_INHERIT, so hashing a
    // DType CLASS during numpy.dtypes registration lands here — is hashable by
    // IDENTITY. Types are always hashable in CPython (`object.__hash__` =
    // _Py_HashPointer); this is the genuine identity hash for a type object, NOT
    // the forbidden fabrication for an unhashable INSTANCE (non-type foreign
    // objects still fall through to the honest hash_not_implemented below).
    if unsafe { PyType_Check(op) } != 0 {
        return unsafe { molt_type_identity_hash(op) };
    }
    unsafe { hash_not_implemented(op) }
}

/// `tp_hash` for `type` objects (the metatype). CPython's `type` inherits
/// `object.__hash__`, i.e. `_Py_HashPointer`: a CLASS is hashable by its
/// identity (address). `numpy.dtypes` registration hashes its DType CLASSES
/// into a dict during `_multiarray_umath` `Py_mod_exec`; a NULL `tp_hash` on
/// `PyType_Type` reports the class "unhashable type: 'type'" and aborts init.
/// This is the genuine CPython identity hash for type objects — NOT the
/// forbidden fabrication of an identity hash for an unhashable INSTANCE (that
/// stays a hard `hash_not_implemented`). numpy's `_DTypeMeta` (tp_base =
/// &PyType_Type) inherits this slot via PyType_Ready, matching CPython.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn molt_type_identity_hash(op: *mut PyObject) -> isize {
    if op.is_null() {
        return -1;
    }
    molt_lang_obj_model::hash_policy::hash_pointer(op as u64) as isize
}

/// Faithful port of CPython `Objects/object.c` `do_richcompare`: reflected
/// (subtype-priority) slot first, then v's slot, then w's; NULL propagates as an
/// error; a both-NotImplemented result resolves EQ/NE by identity and raises
/// TypeError for ordering — never leaks NotImplemented to the caller.
unsafe fn do_richcompare(v: *mut PyObject, w: *mut PyObject, op: c_int) -> *mut PyObject {
    let Some(left) = crate::bridge::observe_pyobject(v) else {
        return ptr::null_mut();
    };
    let Some(right) = crate::bridge::observe_pyobject(w) else {
        return ptr::null_mut();
    };
    if let (
        crate::bridge::ResolvedPyObject::ManagedMolt(left),
        crate::bridge::ResolvedPyObject::ManagedMolt(right),
    ) = (left, right)
    {
        let result = unsafe {
            (crate::hooks::hooks_or_stubs().object_richcompare)(op, left.bits(), right.bits())
        };
        return unsafe { GLOBAL_BRIDGE.owned_result_to_pyobj(result) };
    }
    // Slot dispatch follows semantic `Py_TYPE`, not the physical carrier.
    // Generic managed views deliberately use `MoltManaged_Type` so C code can
    // never read a list/dict/string layout that is not present. The semantic
    // resolver preserves the corresponding builtin type identity and slots.
    let tv = unsafe { crate::bridge::semantic_type_for_resolved(v, left) };
    let tw = unsafe { crate::bridge::semantic_type_for_resolved(w, right) };
    if tv.is_null() || tw.is_null() {
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
        }
        return ptr::null_mut();
    }
    let mut checked_reverse = false;

    // Reflected op on w first when Py_TYPE(w) is a PROPER subtype of Py_TYPE(v).
    if !std::ptr::eq(tv, tw)
        && unsafe { PyType_IsSubtype(tw, tv) } == 1
        && let Some(res) = unsafe { try_slot_richcompare(tw, w, v, swapped_op(op)) }
    {
        checked_reverse = true;
        if res.is_null() {
            return ptr::null_mut();
        }
        if !is_not_implemented(res) {
            return res;
        }
        unsafe { crate::api::refcount::Py_DECREF(res) };
    }
    // v's own slot.
    if let Some(res) = unsafe { try_slot_richcompare(tv, v, w, op) } {
        if res.is_null() {
            return ptr::null_mut();
        }
        if !is_not_implemented(res) {
            return res;
        }
        unsafe { crate::api::refcount::Py_DECREF(res) };
    }
    // w's slot (unless already tried as the reflected op above).
    if !checked_reverse && let Some(res) = unsafe { try_slot_richcompare(tw, w, v, swapped_op(op)) }
    {
        if res.is_null() {
            return ptr::null_mut();
        }
        if !is_not_implemented(res) {
            return res;
        }
        unsafe { crate::api::refcount::Py_DECREF(res) };
    }
    // Neither side handled it: identity for EQ/NE, TypeError for ordering.
    match op {
        CMP_EQ | CMP_NE => {
            let equal = std::ptr::eq(v, w);
            let want = if op == CMP_EQ { equal } else { !equal };
            let res = if want {
                (&raw mut crate::abi_types::Py_True).cast::<PyObject>()
            } else {
                (&raw mut crate::abi_types::Py_False).cast::<PyObject>()
            };
            unsafe { crate::api::refcount::Py_INCREF(res) };
            res
        }
        _ => {
            let msg = format!(
                "'{}' not supported between instances of '{}' and '{}'",
                cmp_opstring(op),
                unsafe { object_type_name_with_precision(v, 100) },
                unsafe { object_type_name_with_precision(w, 100) },
            );
            if let Ok(c) = std::ffi::CString::new(msg) {
                unsafe {
                    crate::api::errors::PyErr_SetString(
                        (&raw mut crate::abi_types::PyExc_TypeError)
                            .cast::<crate::abi_types::PyObject>(),
                        c.as_ptr(),
                    );
                }
            }
            ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_RichCompare(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> *mut PyObject {
    // CPython PyObject_RichCompare: a NULL operand is a BadInternalCall.
    if v.is_null() || w.is_null() || RichCompareOp::from_i32(op).is_none() {
        if unsafe { crate::api::errors::PyErr_Occurred() }.is_null() {
            unsafe { crate::api::errors::PyErr_BadInternalCall() };
        }
        return ptr::null_mut();
    }
    let _runtime_gil = crate::hooks::RuntimeGilGuard::ensure();
    if unsafe { crate::api::memory::Py_EnterRecursiveCall(c" in comparison".as_ptr()) } != 0 {
        return ptr::null_mut();
    }
    let result = unsafe { do_richcompare(v, w, op) };
    unsafe { crate::api::memory::Py_LeaveRecursiveCall() };
    result
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn PyObject_RichCompareBool(
    v: *mut PyObject,
    w: *mut PyObject,
    op: c_int,
) -> c_int {
    // CPython Objects/object.c: identity implies equality — v == w shortcuts
    // EQ->1 / NE->0 BEFORE any slot dispatch (so [nan] == [nan] is True).
    if std::ptr::eq(v, w) && (op == CMP_EQ || op == CMP_NE) {
        // Only this identity shortcut bypasses RichCompare's observation.
        // Every other path admits each operand once in do_richcompare.
        if !v.is_null() && crate::bridge::resolve_pyobject(v).is_none() {
            return -1;
        }
        if op == CMP_EQ {
            return 1;
        } else if op == CMP_NE {
            return 0;
        }
    }
    let res = unsafe { PyObject_RichCompare(v, w, op) };
    if res.is_null() {
        return -1;
    }
    // PyBool_Check fast path, else route the result through PyObject_IsTrue.
    let ok = if std::ptr::eq(res, (&raw mut crate::abi_types::Py_True).cast::<PyObject>()) {
        1
    } else if std::ptr::eq(
        res,
        (&raw mut crate::abi_types::Py_False).cast::<PyObject>(),
    ) {
        0
    } else {
        unsafe { crate::api::object::PyObject_IsTrue(res) }
    };
    unsafe { crate::api::errors::release_preserving_error(&[res]) };
    ok
}

#[cfg(test)]
mod class2_decode_tests {
    use super::*;
    use crate::abi_types::{PyNumberMethods, PyTypeObject};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static HASH_CALLS: AtomicUsize = AtomicUsize::new(0);
    static REPR_CALLS: AtomicUsize = AtomicUsize::new(0);
    static FLOAT_CALLS: AtomicUsize = AtomicUsize::new(0);
    static RICHCOMPARE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static BOOL_CALLS: AtomicUsize = AtomicUsize::new(0);

    static mut REPR_RESULT: PyObject = PyObject {
        ob_refcnt: 1,
        ob_type: ptr::null_mut(),
    };

    unsafe extern "C" fn foreign_hash(_op: *mut PyObject) -> isize {
        HASH_CALLS.fetch_add(1, Ordering::SeqCst);
        4242
    }

    unsafe extern "C" fn foreign_repr(_op: *mut PyObject) -> *mut PyObject {
        REPR_CALLS.fetch_add(1, Ordering::SeqCst);
        &raw mut REPR_RESULT
    }

    unsafe extern "C" fn foreign_float(_op: *mut PyObject) -> *mut PyObject {
        FLOAT_CALLS.fetch_add(1, Ordering::SeqCst);
        unsafe { crate::api::numbers::PyFloat_FromDouble(42.5) }
    }

    unsafe extern "C" fn foreign_richcompare(
        _left: *mut PyObject,
        _right: *mut PyObject,
        _op: c_int,
    ) -> *mut PyObject {
        RICHCOMPARE_CALLS.fetch_add(1, Ordering::SeqCst);
        unsafe { crate::api::object::Py_NewRef((&raw mut crate::abi_types::Py_True).cast()) }
    }

    unsafe extern "C" fn foreign_bool(_op: *mut PyObject) -> c_int {
        BOOL_CALLS.fetch_add(1, Ordering::SeqCst);
        0
    }

    #[test]
    fn every_stable_slot_has_one_symmetric_storage_authority() {
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        crate::bridge::molt_cpython_abi_init();
        for id in 1..=81 {
            unsafe { crate::api::errors::PyErr_Clear() };
            let wrapper = stable_slot_wrapper(id).expect("all public slot ids are mapped");
            let mut heap: PyHeapTypeObject = unsafe { std::mem::zeroed() };
            let ty = &raw mut heap.ht_type;

            // A valid but unset slot returns NULL without manufacturing an error,
            // including a protocol slot whose parent table does not yet exist.
            assert!(unsafe { PyType_GetSlot(ty, id) }.is_null());
            assert!(unsafe { crate::api::errors::PyErr_Occurred() }.is_null());

            assert!(record_native_heap_type_allocation(
                ty.addr(),
                std::mem::size_of::<PyHeapTypeObject>()
            ));
            unsafe {
                inheritance::prepare_layout(ty);
            }
            let storage = unsafe { slot_wrapper_storage(ty, wrapper) };
            assert!(!storage.is_null());
            let sentinel = std::ptr::without_provenance_mut::<c_void>(0x1000 + id as usize * 16);
            unsafe { storage.write(sentinel) };
            assert_eq!(unsafe { PyType_GetSlot(ty, id) }, sentinel);
            assert!(unsafe { crate::api::errors::PyErr_Occurred() }.is_null());

            unregister_type_address(ty.addr());
        }
        for id in [-1, 0, 82, i32::MAX] {
            unsafe { crate::api::errors::PyErr_Clear() };
            let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
            assert!(unsafe { PyType_GetSlot(&raw mut ty, id) }.is_null());
            assert!(!unsafe { crate::api::errors::PyErr_Occurred() }.is_null());
        }
        unsafe { crate::api::errors::PyErr_Clear() };
    }

    #[test]
    fn raw_foreign_object_never_decodes_as_molt_value() {
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        crate::bridge::molt_cpython_abi_init();
        HASH_CALLS.store(0, Ordering::SeqCst);
        REPR_CALLS.store(0, Ordering::SeqCst);
        FLOAT_CALLS.store(0, Ordering::SeqCst);
        RICHCOMPARE_CALLS.store(0, Ordering::SeqCst);
        BOOL_CALLS.store(0, Ordering::SeqCst);

        let mut number: PyNumberMethods = unsafe { std::mem::zeroed() };
        number.nb_float = foreign_float as *mut c_void;
        number.nb_bool = foreign_bool as *mut c_void;
        let mut ty: PyTypeObject = unsafe { std::mem::zeroed() };
        ty.tp_name = c"numpy_like_foreign".as_ptr();
        ty.tp_hash = Some(foreign_hash);
        ty.tp_repr = Some(foreign_repr);
        ty.tp_richcompare = Some(foreign_richcompare);
        ty.tp_as_number = (&raw mut number).cast();
        let mut obj = PyObject {
            ob_refcnt: 1,
            ob_type: &raw mut ty,
        };
        unsafe {
            REPR_RESULT.ob_type = &raw mut crate::abi_types::PyUnicode_Type;
        }

        assert_eq!(unsafe { PyObject_Hash(&raw mut obj) }, 4242);
        assert_eq!(HASH_CALLS.load(Ordering::SeqCst), 1);

        assert_eq!(unsafe { PyObject_Repr(&raw mut obj) }, &raw mut REPR_RESULT);
        assert_eq!(REPR_CALLS.load(Ordering::SeqCst), 1);

        assert_eq!(
            unsafe { crate::api::numbers::PyFloat_AsDouble(&raw mut obj) },
            42.5
        );
        assert_eq!(FLOAT_CALLS.load(Ordering::SeqCst), 1);

        assert_eq!(
            unsafe { PyObject_RichCompare(&raw mut obj, &raw mut obj, CMP_EQ) },
            (&raw mut crate::abi_types::Py_True).cast()
        );
        assert_eq!(RICHCOMPARE_CALLS.load(Ordering::SeqCst), 1);

        // RichCompare must invoke even a same-object slot; only its Bool
        // consumer owns the intentional identity shortcut.
        assert_eq!(
            unsafe { PyObject_RichCompareBool(&raw mut obj, &raw mut obj, CMP_EQ) },
            1
        );
        assert_eq!(
            unsafe { PyObject_RichCompareBool(&raw mut obj, &raw mut obj, CMP_NE) },
            0
        );
        assert_eq!(RICHCOMPARE_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(
            unsafe { PyObject_RichCompare(&raw mut obj, &raw mut obj, CMP_NE) },
            (&raw mut crate::abi_types::Py_True).cast()
        );
        assert_eq!(RICHCOMPARE_CALLS.load(Ordering::SeqCst), 2);

        assert_eq!(
            unsafe { crate::api::object::PyObject_IsTrue(&raw mut obj) },
            0
        );
        assert_eq!(BOOL_CALLS.load(Ordering::SeqCst), 1);

        assert_eq!(
            crate::bridge::GLOBAL_BRIDGE.release_pyobj(&raw mut obj),
            crate::bridge::PyObjRelease::Untracked
        );
    }
}

#[cfg(test)]
pub(crate) mod subclass_registry_tests {
    use super::*;
    use crate::abi_types::{
        Py_TPFLAGS_VALID_VERSION_TAG, PyTuple_Type, PyTupleObject, PyVarObject,
    };

    // Watcher slots are process-global. Hold this through registration,
    // callbacks and cleanup so the default parallel test runner cannot steal
    // a cleared lower slot from another watcher's dispatch test.
    static WATCHER_TEST_LOCK: Mutex<()> = Mutex::new(());

    // Bridge retirement tests use the real non-owning registry without
    // dereferencing freed storage or adding a production introspection API.
    pub(crate) unsafe fn register_subclass_for_test(
        base: *mut PyTypeObject,
        child: *mut PyTypeObject,
    ) {
        unsafe { register_subclass(base, child) };
    }

    pub(crate) fn assert_type_address_retired(address: usize) {
        let registry = TYPE_SUBCLASSES.lock();
        assert!(!registry.live.contains_key(&address));
        for (base, children) in &registry.subclasses {
            assert_ne!(base.address, address);
            assert!(
                children
                    .members
                    .iter()
                    .all(|child| child.address != address)
            );
        }
        for (child, bases) in &registry.bases_by_subclass {
            assert_ne!(child.address, address);
            assert!(bases.iter().all(|base| base.address != address));
        }
    }

    #[repr(C)]
    struct RawTuple2 {
        base: PyTupleObject,
        second: *mut PyObject,
    }

    fn raw_tuple(items: &[*mut PyTypeObject]) -> RawTuple2 {
        assert!(!items.is_empty() && items.len() <= 2);
        RawTuple2 {
            base: PyTupleObject {
                ob_base: PyVarObject {
                    ob_base: PyObject {
                        ob_refcnt: 1,
                        ob_type: &raw mut PyTuple_Type,
                    },
                    ob_size: items.len() as Py_ssize_t,
                },
                ob_item: [items[0].cast()],
            },
            second: items.get(1).copied().unwrap_or(ptr::null_mut()).cast(),
        }
    }

    fn blank_type(refcnt: isize) -> Box<PyTypeObject> {
        let mut ty: Box<PyTypeObject> = Box::new(unsafe { std::mem::zeroed() });
        ty.ob_base.ob_base.ob_refcnt = refcnt;
        ty.tp_flags = Py_TPFLAGS_READY | Py_TPFLAGS_VALID_VERSION_TAG;
        ty.tp_version_tag = 41;
        ty
    }

    #[test]
    fn fresh_type_allocation_replaces_stale_identity_and_all_subclass_edges() {
        for already_has_extent in [false, true] {
            let mut base = blank_type(1);
            let mut descendant = blank_type(1);
            let mut heap: Box<PyHeapTypeObject> = Box::new(unsafe { std::mem::zeroed() });
            let pointer = &raw mut heap.ht_type;
            if already_has_extent {
                assert!(record_native_heap_type_allocation(
                    pointer.addr(),
                    std::mem::size_of::<PyHeapTypeObject>()
                ));
            }
            unsafe {
                register_subclass(&raw mut *base, pointer);
                register_subclass(pointer, &raw mut *descendant);
            }
            let old_generation = TYPE_SUBCLASSES.lock().live[&pointer.addr()].generation;
            // Model allocator address reuse without dereferencing freed memory.
            assert!(record_native_heap_type_allocation(
                pointer.addr(),
                std::mem::size_of::<PyHeapTypeObject>()
            ));
            {
                let registry = TYPE_SUBCLASSES.lock();
                assert_ne!(registry.live[&pointer.addr()].generation, old_generation);
                let old_identity = TypeIdentity {
                    address: pointer.addr(),
                    generation: old_generation,
                };
                assert!(!registry.subclasses.contains_key(&old_identity));
                assert!(!registry.bases_by_subclass.contains_key(&old_identity));
                assert!(
                    registry
                        .subclasses
                        .values()
                        .all(|entry| { !entry.members.contains(&old_identity) })
                );
                assert!(
                    registry
                        .bases_by_subclass
                        .values()
                        .all(|bases| { !bases.contains(&old_identity) })
                );
            }
            unregister_type_address(pointer.addr());
            unregister_type_address((&raw mut *base).addr());
            unregister_type_address((&raw mut *descendant).addr());
        }
    }

    #[test]
    fn spec_extent_admission_preserves_current_identity_and_subclass_edges() {
        let mut base = blank_type(1);
        let mut descendant = blank_type(1);
        let mut heap: Box<PyHeapTypeObject> = Box::new(unsafe { std::mem::zeroed() });
        let pointer = &raw mut heap.ht_type;
        unsafe {
            register_subclass(&raw mut *base, pointer);
            register_subclass(pointer, &raw mut *descendant);
        }
        let old_generation = TYPE_SUBCLASSES.lock().live[&pointer.addr()].generation;
        for _ in 0..2 {
            assert_eq!(
                admit_spec_type_allocation(pointer.addr(), std::mem::size_of::<PyHeapTypeObject>()),
                Ok(())
            );
            let registry = TYPE_SUBCLASSES.lock();
            assert_eq!(registry.live[&pointer.addr()].generation, old_generation);
            let identity = TypeIdentity {
                address: pointer.addr(),
                generation: old_generation,
            };
            assert_eq!(registry.subclasses[&identity].members.len(), 1);
            assert_eq!(registry.bases_by_subclass[&identity].len(), 1);
        }
        unregister_type_address(pointer.addr());
        unregister_type_address((&raw mut *base).addr());
        unregister_type_address((&raw mut *descendant).addr());
    }

    #[test]
    fn type_storage_receipt_is_generation_owned_and_flag_independent() {
        let mut heap: Box<PyHeapTypeObject> = Box::new(unsafe { std::mem::zeroed() });
        let pointer = (&raw mut *heap).cast::<PyTypeObject>();
        assert!(heap_type_storage(pointer).is_none());
        unsafe {
            (*pointer).tp_flags = Py_TPFLAGS_HEAPTYPE;
        }
        assert!(
            heap_type_storage(pointer).is_none(),
            "a public flag is not allocation evidence"
        );
        assert!(record_native_heap_type_allocation(
            pointer.addr(),
            std::mem::size_of::<PyHeapTypeObject>()
        ));
        unsafe {
            (*pointer).tp_flags = 0;
        }
        assert_eq!(heap_type_storage(pointer), Some(&raw mut *heap));
        assert_eq!(
            admit_spec_type_allocation(pointer.addr(), std::mem::size_of::<PyHeapTypeObject>() + 1),
            Err(TypeStorageAdmissionError::InsufficientExtent)
        );
        unregister_type_address(pointer.addr());
        assert!(
            heap_type_storage(pointer).is_none(),
            "address reuse cannot retain extent"
        );
    }

    #[test]
    fn static_retirement_detaches_the_whole_cohort_before_releasing_owned_edges() {
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        #[repr(C)]
        struct RootProbe {
            object: PyObject,
            shells: [*mut PyTypeObject; 4],
            calls: usize,
            observed_detachment: bool,
            rejected_reentry: bool,
        }
        unsafe extern "C" fn observe_detachment(object: *mut PyObject) {
            let probe = unsafe { &mut *object.cast::<RootProbe>() };
            probe.calls += 1;
            let registry = TYPE_SUBCLASSES.lock();
            probe.observed_detachment = probe.shells.iter().all(|&tp| unsafe {
                (*tp).tp_dict.is_null()
                    && (*tp).tp_bases.is_null()
                    && (*tp).tp_mro.is_null()
                    && (*tp).tp_cache.is_null()
                    && !registry.live.contains_key(&tp.addr())
            });
            drop(registry);
            probe.rejected_reentry = probe.shells.iter().all(|&tp| unsafe {
                let rejected =
                    PyType_Ready(tp) == -1 && !crate::api::errors::PyErr_Occurred().is_null();
                crate::api::errors::PyErr_Clear();
                rejected
            });
        }

        let mut first = blank_type(crate::abi_types::IMMORTAL_REFCNT);
        let mut second = blank_type(crate::abi_types::IMMORTAL_REFCNT);
        let mut bootstrap = blank_type(crate::abi_types::IMMORTAL_REFCNT);
        let mut empty_exception = blank_type(crate::abi_types::IMMORTAL_REFCNT);
        let mut unrelated_base = blank_type(17);
        let mut unrelated_child = blank_type(23);
        let shells = [
            &raw mut *first,
            &raw mut *second,
            &raw mut *bootstrap,
            &raw mut *empty_exception,
        ];
        let base = &raw mut *unrelated_base;
        let child = &raw mut *unrelated_child;
        let mut root_type = blank_type(1);
        root_type.tp_dealloc = Some(observe_detachment);
        let mut early = RootProbe {
            object: PyObject {
                ob_refcnt: 1,
                ob_type: &raw mut *root_type,
            },
            shells,
            calls: 0,
            observed_detachment: false,
            rejected_reentry: false,
        };
        let mut shared = RootProbe {
            object: PyObject {
                ob_refcnt: 7,
                ob_type: &raw mut *root_type,
            },
            shells,
            calls: 0,
            observed_detachment: false,
            rejected_reentry: false,
        };
        first.tp_dict = (&raw mut early).cast();
        first.tp_bases = (&raw mut shared).cast();
        first.tp_mro = (&raw mut shared).cast();
        first.tp_cache = (&raw mut shared).cast();
        second.tp_dict = (&raw mut shared).cast();
        second.tp_bases = (&raw mut shared).cast();
        second.tp_mro = (&raw mut shared).cast();
        second.tp_cache = (&raw mut shared).cast();
        first.tp_base = base;
        let type_name = c"retirement_probe".as_ptr();
        first.tp_name = type_name;
        first.tp_basicsize = 123;
        first.tp_dealloc = Some(observe_detachment);
        first.tp_flags |=
            crate::abi_types::Py_TPFLAGS_READYING | crate::abi_types::Py_TPFLAGS_IMMUTABLETYPE;
        first.tp_watched = 3;
        unsafe {
            for shell in shells {
                register_subclass(base, shell);
            }
            register_subclass(shells[0], shells[1]);
            register_subclass(shells[1], child);
            register_subclass(base, child);
        }
        let (old_generation, base_identity, child_identity) = {
            let registry = TYPE_SUBCLASSES.lock();
            (
                registry.live[&shells[0].addr()].generation,
                TypeIdentity {
                    address: base.addr(),
                    generation: registry.live[&base.addr()].generation,
                },
                TypeIdentity {
                    address: child.addr(),
                    generation: registry.live[&child.addr()].generation,
                },
            )
        };
        let version_counter = NEXT_TYPE_VERSION_TAG.load(Ordering::Relaxed);
        let cohort = [
            (shells[0], false),
            (shells[1], true),
            (shells[2], true),
            (shells[3], false),
            (shells[0], false),
            (ptr::null_mut(), false),
        ];
        unsafe { retire_static_type_runtime_roots(&cohort) };

        assert_eq!((early.calls, shared.calls), (1, 1));
        assert_eq!((early.object.ob_refcnt, shared.object.ob_refcnt), (0, 0));
        assert!(early.observed_detachment && shared.observed_detachment);
        assert!(early.rejected_reentry && shared.rejected_reentry);
        assert_eq!(first.tp_flags, crate::abi_types::Py_TPFLAGS_IMMUTABLETYPE);
        assert_eq!(second.tp_flags, 0);
        assert_eq!(bootstrap.tp_flags, Py_TPFLAGS_READY);
        assert_eq!(empty_exception.tp_flags, 0);
        assert_eq!((first.tp_version_tag, first.tp_watched), (0, 0));
        assert_eq!(first.tp_base, base);
        assert_eq!(first.tp_name, type_name);
        assert_eq!(first.tp_basicsize, 123);
        assert!(std::ptr::fn_addr_eq(
            first.tp_dealloc.unwrap(),
            observe_detachment as unsafe extern "C" fn(*mut PyObject)
        ));
        assert_eq!(
            first.ob_base.ob_base.ob_refcnt,
            crate::abi_types::IMMORTAL_REFCNT
        );
        {
            let registry = TYPE_SUBCLASSES.lock();
            assert_eq!(
                registry.live[&base.addr()].generation,
                base_identity.generation
            );
            assert_eq!(
                registry.live[&child.addr()].generation,
                child_identity.generation
            );
            assert_eq!(
                registry.subclasses[&base_identity].members,
                HashSet::from([child_identity])
            );
            assert_eq!(
                registry.bases_by_subclass[&child_identity],
                HashSet::from([base_identity])
            );
        }
        unsafe { retire_static_type_runtime_roots(&cohort) };
        assert_eq!(
            (early.calls, shared.calls),
            (1, 1),
            "retirement is idempotent"
        );
        assert_eq!(unsafe { PyType_Ready(shells[2]) }, -1);
        unsafe { crate::api::errors::PyErr_Clear() };
        assert!(!unsafe { assign_type_version_tag(shells[2], &mut HashSet::new()) });
        {
            let mut registry = TYPE_SUBCLASSES.lock();
            assert!(type_identity(&mut registry, shells[0]).is_none());
        }
        reopen_static_type_runtime_roots(&cohort);
        let new_generation = {
            let mut registry = TYPE_SUBCLASSES.lock();
            type_identity(&mut registry, shells[0]).unwrap().generation
        };
        assert!(new_generation > old_generation);
        assert!(NEXT_TYPE_VERSION_TAG.load(Ordering::Relaxed) >= version_counter);
        assert!(unsafe { assign_type_version_tag(shells[2], &mut HashSet::new()) });

        // A subsequent bootstrap can fail after publishing only a dict. Its
        // unfinished roots and READYING state follow the same retirement path.
        shared.object.ob_refcnt = 1;
        first.tp_dict = (&raw mut shared).cast();
        first.tp_flags |= crate::abi_types::Py_TPFLAGS_READYING;
        unsafe { retire_static_type_runtime_roots(&cohort) };
        assert_eq!((early.calls, shared.calls), (1, 2));
        assert!(shared.observed_detachment && shared.rejected_reentry);
        assert_eq!(first.tp_flags, crate::abi_types::Py_TPFLAGS_IMMUTABLETYPE);
        reopen_static_type_runtime_roots(&cohort);
        unregister_type_address(shells[0].addr());
        unregister_type_address(base.addr());
        unregister_type_address(child.addr());
    }

    #[test]
    fn recursive_type_readiness_fails_without_consuming_the_outer_guard() {
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        let mut ty = blank_type(crate::abi_types::IMMORTAL_REFCNT);
        ty.tp_flags =
            crate::abi_types::Py_TPFLAGS_IMMUTABLETYPE | crate::abi_types::Py_TPFLAGS_READYING;
        {
            let _outer = TypeReadyingGuard(&raw mut *ty);
            assert_eq!(unsafe { PyType_Ready(&raw mut *ty) }, -1);
            assert!(!unsafe { crate::api::errors::PyErr_Occurred() }.is_null());
            assert!(ty.tp_dict.is_null() && ty.tp_mro.is_null());
            assert_ne!(ty.tp_flags & crate::abi_types::Py_TPFLAGS_READYING, 0);
            unsafe { crate::api::errors::PyErr_Clear() };
        }
        assert_eq!(ty.tp_flags, crate::abi_types::Py_TPFLAGS_IMMUTABLETYPE);
    }

    #[test]
    fn subclass_registry_is_non_owning_and_address_reuse_gets_new_generation() {
        let mut base = blank_type(17);
        let mut child = blank_type(23);
        let base_ptr = &raw mut *base;
        let child_ptr = &raw mut *child;

        unsafe { register_subclass(base_ptr, child_ptr) };
        assert_eq!(base.ob_base.ob_base.ob_refcnt, 17);
        assert_eq!(child.ob_base.ob_base.ob_refcnt, 23);
        let old_identity = {
            let registry = TYPE_SUBCLASSES.lock();
            TypeIdentity {
                address: child_ptr.addr(),
                generation: registry.live[&child_ptr.addr()].generation,
            }
        };

        unregister_type_address(child_ptr.addr());
        let new_identity = {
            let mut registry = TYPE_SUBCLASSES.lock();
            type_identity(&mut registry, child_ptr).expect("re-registered type identity")
        };

        assert_ne!(new_identity, old_identity);
        assert_eq!(base.ob_base.ob_base.ob_refcnt, 17);
        assert_eq!(child.ob_base.ob_base.ob_refcnt, 23);
        unregister_type_address(child_ptr.addr());
        unregister_type_address(base_ptr.addr());
    }

    #[repr(C)]
    struct RevokingWatcherType {
        ty: PyTypeObject,
        victim: *mut PyTypeObject,
        replacement_base: *mut PyTypeObject,
        calls: usize,
        callback_refcnt: isize,
        clear_watcher: c_int,
        replace_cleared_watcher: bool,
        watcher_status: c_int,
        replacement_calls: usize,
        zero_victim_refcnt: bool,
        deallocations: usize,
    }

    impl RevokingWatcherType {
        fn new() -> Box<Self> {
            Box::new(Self {
                ty: *blank_type(1),
                victim: ptr::null_mut(),
                replacement_base: ptr::null_mut(),
                calls: 0,
                callback_refcnt: 0,
                clear_watcher: -1,
                replace_cleared_watcher: false,
                watcher_status: -1,
                replacement_calls: 0,
                zero_victim_refcnt: false,
                deallocations: 0,
            })
        }
    }

    unsafe extern "C" fn revoke_watched_type(object: *mut PyObject) -> c_int {
        let watched = unsafe { &mut *object.cast::<RevokingWatcherType>() };
        watched.calls += 1;
        watched.callback_refcnt = watched.ty.ob_base.ob_base.ob_refcnt;
        if watched.clear_watcher >= 0 {
            watched.watcher_status = unsafe { PyType_ClearWatcher(watched.clear_watcher) };
            if watched.watcher_status == 0 && watched.replace_cleared_watcher {
                watched.watcher_status =
                    unsafe { PyType_AddWatcher(Some(replacement_type_watcher)) };
            }
        }
        if !watched.victim.is_null() {
            if watched.zero_victim_refcnt {
                unsafe { (*watched.victim).ob_base.ob_base.ob_refcnt = 0 };
                return 0;
            }
            unregister_type_address(watched.victim.addr());
            if !watched.replacement_base.is_null() {
                // Model allocation reuse while keeping test storage valid.
                // The queued old generation must not act on its replacement.
                unsafe { register_subclass(watched.replacement_base, watched.victim) };
            }
        }
        0
    }

    unsafe extern "C" fn replacement_type_watcher(object: *mut PyObject) -> c_int {
        let watched = unsafe { &mut *object.cast::<RevokingWatcherType>() };
        watched.replacement_calls += 1;
        0
    }

    #[test]
    fn watcher_dispatch_observes_later_slot_clear_and_replacement() {
        let _watcher_lock = WATCHER_TEST_LOCK.lock();
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        for replace in [false, true] {
            let first = unsafe { PyType_AddWatcher(Some(revoke_watched_type)) };
            let second = unsafe { PyType_AddWatcher(Some(revoke_watched_type)) };
            assert!(first >= 0 && second > first);
            let mut watched = RevokingWatcherType::new();
            let pointer = &raw mut watched.ty;
            watched.clear_watcher = second;
            watched.replace_cleared_watcher = replace;
            watched.ty.tp_watched = (1 << first) | (1 << second);
            unsafe { PyType_Modified(pointer) };
            assert_eq!(watched.calls, 1, "the old later callback must not run");
            assert_eq!(watched.replacement_calls, usize::from(replace));
            assert_eq!(watched.watcher_status, if replace { second } else { 0 });
            assert_eq!(watched.ty.ob_base.ob_base.ob_refcnt, 1);
            assert_eq!(watched.ty.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
            assert_eq!(watched.ty.tp_version_tag, 0);
            unregister_type_address(pointer.addr());
            assert_eq!(unsafe { PyType_ClearWatcher(first) }, 0);
            if replace {
                assert_eq!(unsafe { PyType_ClearWatcher(second) }, 0);
            }
        }
    }

    #[test]
    fn queued_type_invalidation_skips_retired_and_reused_generations_in_both_phases() {
        let _watcher_lock = WATCHER_TEST_LOCK.lock();
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        let watcher = unsafe { PyType_AddWatcher(Some(revoke_watched_type)) };
        assert!(watcher >= 0);
        for expanded_victim in [false, true] {
            for reuse in [false, true] {
                let mut base = RevokingWatcherType::new();
                let mut first = RevokingWatcherType::new();
                let mut second = RevokingWatcherType::new();
                let base_ptr = &raw mut base.ty;
                let first_ptr = &raw mut first.ty;
                let second_ptr = &raw mut second.ty;
                let victim = if expanded_victim {
                    base_ptr
                } else {
                    second_ptr
                };
                for ty in [base_ptr, first_ptr, second_ptr] {
                    unsafe { (*ty).tp_watched = 1 << watcher };
                }
                first.victim = victim;
                first.replacement_base = if reuse { first_ptr } else { ptr::null_mut() };
                unsafe {
                    register_subclass(base_ptr, first_ptr);
                    register_subclass(base_ptr, second_ptr);
                }
                let old_generation = TYPE_SUBCLASSES.lock().live[&victim.addr()].generation;
                unsafe { PyType_Modified(base_ptr) };
                let target = if expanded_victim { &base } else { &second };
                assert_eq!(target.calls, 0, "a stale queued identity reached a watcher");
                assert_ne!(target.ty.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
                assert_eq!(target.ty.tp_version_tag, 41);
                assert_eq!(target.ty.ob_base.ob_base.ob_refcnt, 1);
                assert_eq!(first.calls, 1);
                assert_eq!(first.callback_refcnt, 2, "callback needs a live owner");
                assert_eq!(first.ty.ob_base.ob_base.ob_refcnt, 1);
                {
                    let registry = TYPE_SUBCLASSES.lock();
                    if reuse {
                        assert_ne!(registry.live[&victim.addr()].generation, old_generation);
                    } else {
                        assert!(!registry.live.contains_key(&victim.addr()));
                    }
                }
                for ty in [base_ptr, first_ptr, second_ptr] {
                    unregister_type_address(ty.addr());
                }
            }
        }
        assert_eq!(unsafe { PyType_ClearWatcher(watcher) }, 0);
    }

    #[test]
    fn watcher_retirement_stops_remaining_callbacks_and_invalidation() {
        let _watcher_lock = WATCHER_TEST_LOCK.lock();
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        let first = unsafe { PyType_AddWatcher(Some(revoke_watched_type)) };
        let second = unsafe { PyType_AddWatcher(Some(revoke_watched_type)) };
        assert!(first >= 0 && second > first);
        let mut watched = RevokingWatcherType::new();
        let pointer = &raw mut watched.ty;
        watched.victim = pointer;
        watched.ty.tp_watched = (1 << first) | (1 << second);
        unsafe { PyType_Modified(pointer) };
        assert_eq!(watched.calls, 1);
        assert_eq!(watched.callback_refcnt, 2);
        assert_eq!(watched.ty.ob_base.ob_base.ob_refcnt, 1);
        assert_ne!(watched.ty.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
        assert_eq!(watched.ty.tp_version_tag, 41);
        assert_type_address_retired(pointer.addr());
        assert_eq!(unsafe { PyType_ClearWatcher(first) }, 0);
        assert_eq!(unsafe { PyType_ClearWatcher(second) }, 0);
    }

    unsafe extern "C" fn count_watched_type_deallocation(object: *mut PyObject) {
        unsafe { (*object.cast::<RevokingWatcherType>()).deallocations += 1 };
    }

    #[test]
    fn invalidation_never_retains_a_dying_type_in_either_traversal_phase() {
        let _watcher_lock = WATCHER_TEST_LOCK.lock();
        let _thread_state = crate::api::object::AbiTestThreadStateTransaction::new();
        let watcher = unsafe { PyType_AddWatcher(Some(revoke_watched_type)) };
        assert!(watcher >= 0);
        let mut metatype = blank_type(crate::abi_types::IMMORTAL_REFCNT);
        metatype.tp_dealloc = Some(count_watched_type_deallocation);
        for dying_after_expansion in [false, true] {
            let mut base = RevokingWatcherType::new();
            let mut sibling = RevokingWatcherType::new();
            let mut child = RevokingWatcherType::new();
            let base_ptr = &raw mut base.ty;
            let sibling_ptr = &raw mut sibling.ty;
            let child_ptr = &raw mut child.ty;
            for tp in [base_ptr, sibling_ptr, child_ptr] {
                unsafe {
                    (*tp).tp_watched = 1 << watcher;
                    (*tp).ob_base.ob_base.ob_type = &raw mut *metatype;
                }
            }
            let dying = if dying_after_expansion {
                sibling.victim = base_ptr;
                sibling.zero_victim_refcnt = true;
                base_ptr
            } else {
                child.ty.ob_base.ob_base.ob_refcnt = 0;
                child_ptr
            };
            unsafe {
                register_subclass(base_ptr, sibling_ptr);
                register_subclass(base_ptr, child_ptr);
                PyType_Modified(base_ptr);
            }
            let target = if dying_after_expansion { &base } else { &child };
            assert_eq!(target.calls, 0, "a dying type reached a watcher");
            assert_eq!(target.deallocations, 0, "watching reentered deallocation");
            assert_eq!(target.ty.ob_base.ob_base.ob_refcnt, 0);
            assert_ne!(target.ty.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
            assert_eq!(sibling.calls, 1);
            assert_eq!(sibling.callback_refcnt, 2);
            assert_eq!(sibling.ty.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
            assert!(TYPE_SUBCLASSES.lock().live.contains_key(&dying.addr()));
            for tp in [base_ptr, sibling_ptr, child_ptr] {
                unregister_type_address(tp.addr());
            }
        }
        assert_eq!(unsafe { PyType_ClearWatcher(watcher) }, 0);
    }

    #[test]
    fn subclass_retirement_bounds_tombstones_without_base_invalidation() {
        let mut base = blank_type(1);
        let mut survivor = blank_type(1);
        let mut transient = blank_type(1);
        let base_ptr = &raw mut *base;
        let survivor_ptr = &raw mut *survivor;
        let transient_ptr = &raw mut *transient;
        unsafe { register_subclass(base_ptr, survivor_ptr) };
        let (base_identity, survivor_identity) = {
            let mut registry = TYPE_SUBCLASSES.lock();
            (
                type_identity(&mut registry, base_ptr).unwrap(),
                type_identity(&mut registry, survivor_ptr).unwrap(),
            )
        };
        // Repeated address reuse must neither accumulate dead generations nor
        // reorder the survivor, even for a base that is never modified.
        for _ in 0..10_000 {
            unsafe { register_subclass(base_ptr, transient_ptr) };
            let retired_identity = {
                let mut registry = TYPE_SUBCLASSES.lock();
                type_identity(&mut registry, transient_ptr).unwrap()
            };
            unregister_type_address(transient_ptr.addr());
            let registry = TYPE_SUBCLASSES.lock();
            let children = &registry.subclasses[&base_identity];
            assert_eq!(children.members.len(), 1);
            assert_eq!(children.order[0], survivor_identity);
            assert!(children.order.len() <= 2);
            assert!(!registry.bases_by_subclass.contains_key(&retired_identity));
        }
        unregister_type_address(survivor_ptr.addr());
        assert!(
            !TYPE_SUBCLASSES
                .lock()
                .subclasses
                .contains_key(&base_identity)
        );
        assert_ne!(base.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG, 0);
        unregister_type_address(base_ptr.addr());
    }

    #[test]
    fn subclass_retirement_reclaims_cohort_storage_and_preserves_registration_order() {
        const WIDTH: usize = 2048;
        let mut base = blank_type(1);
        let base_ptr = &raw mut *base;
        let mut children: Vec<_> = (0..WIDTH).map(|_| blank_type(1)).collect();
        for child in &mut children {
            unsafe { register_subclass(base_ptr, &raw mut **child) };
        }
        let base_identity = {
            let mut registry = TYPE_SUBCLASSES.lock();
            type_identity(&mut registry, base_ptr).unwrap()
        };
        for child in children.iter_mut().take(WIDTH - 3) {
            unregister_type_address((&raw mut **child).addr());
        }
        {
            let registry = TYPE_SUBCLASSES.lock();
            let entry = &registry.subclasses[&base_identity];
            let live_order: Vec<_> = entry
                .order
                .iter()
                .filter(|id| entry.members.contains(id))
                .map(|id| id.address)
                .collect();
            let expected: Vec<_> = children[WIDTH - 3..]
                .iter()
                .map(|child| (&**child as *const PyTypeObject).addr())
                .collect();
            assert_eq!(live_order, expected);
            assert!(entry.order.len() <= 2 * entry.members.len());
            assert!(entry.order.capacity() <= 8 * entry.members.len());
            assert!(entry.members.capacity() <= 8 * entry.members.len());
        }
        for child in &mut children[WIDTH - 3..] {
            unregister_type_address((&raw mut **child).addr());
        }
        assert!(
            !TYPE_SUBCLASSES
                .lock()
                .subclasses
                .contains_key(&base_identity)
        );
        unregister_type_address(base_ptr.addr());
    }

    #[test]
    fn dead_heavy_subclass_registry_compacts_in_one_linear_pass() {
        const WIDTH: usize = 2048;
        let mut base = blank_type(1);
        let base_ptr = &raw mut *base;
        let mut children: Vec<_> = (0..WIDTH).map(|_| blank_type(1)).collect();
        for child in &mut children {
            unsafe { register_subclass(base_ptr, &raw mut **child) };
        }
        for child in children.iter_mut().step_by(2) {
            unregister_type_address((&raw mut **child).addr());
        }

        unsafe { PyType_Modified(base_ptr) };

        for (index, child) in children.iter().enumerate() {
            let is_valid = child.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG != 0;
            assert_eq!(is_valid, index % 2 == 0);
        }
        let base_identity = {
            let registry = TYPE_SUBCLASSES.lock();
            TypeIdentity {
                address: base_ptr.addr(),
                generation: registry.live[&base_ptr.addr()].generation,
            }
        };
        assert_eq!(
            TYPE_SUBCLASSES.lock().subclasses[&base_identity]
                .order
                .len(),
            WIDTH / 2
        );
        for child in &mut children {
            unregister_type_address((&raw mut **child).addr());
        }
        unregister_type_address(base_ptr.addr());
    }

    #[test]
    fn deep_hierarchy_invalidation_is_iterative_in_registry_width() {
        const DEPTH: usize = 16_384;
        let mut types: Vec<_> = (0..DEPTH).map(|_| blank_type(1)).collect();
        for index in 1..types.len() {
            let base = &raw mut *types[index - 1];
            let child = &raw mut *types[index];
            unsafe { register_subclass(base, child) };
        }
        unsafe { PyType_Modified(&raw mut *types[0]) };
        assert!(
            types
                .iter()
                .all(|ty| ty.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG == 0)
        );
        for ty in &mut types {
            unregister_type_address((&raw mut **ty).addr());
        }
    }

    #[test]
    fn concurrent_subclass_registration_has_one_edge_per_child() {
        const WIDTH: usize = 1024;
        const THREADS: usize = 8;
        let mut base = blank_type(1);
        let base_address = (&raw mut *base).addr();
        let mut children: Vec<_> = (0..WIDTH).map(|_| blank_type(1)).collect();
        let child_addresses: Vec<_> = children
            .iter_mut()
            .map(|child| (&raw mut **child).addr())
            .collect();
        std::thread::scope(|scope| {
            for shard in 0..THREADS {
                let addresses = &child_addresses;
                scope.spawn(move || {
                    for child_address in addresses.iter().skip(shard).step_by(THREADS) {
                        unsafe {
                            register_subclass(
                                ptr::with_exposed_provenance_mut(base_address),
                                ptr::with_exposed_provenance_mut(*child_address),
                            )
                        };
                    }
                });
            }
        });
        let registry = TYPE_SUBCLASSES.lock();
        let base_identity = TypeIdentity {
            address: base_address,
            generation: registry.live[&base_address].generation,
        };
        assert_eq!(registry.subclasses[&base_identity].order.len(), WIDTH);
        drop(registry);
        for address in child_addresses {
            unregister_type_address(address);
        }
        unregister_type_address(base_address);
    }

    #[test]
    fn version_tags_accept_diamonds_reject_cycles_and_never_wrap() {
        let mut root = blank_type(1);
        let mut left = blank_type(1);
        let mut right = blank_type(1);
        let mut diamond = blank_type(1);
        for ty in [&mut root, &mut left, &mut right, &mut diamond] {
            ty.tp_flags &= !Py_TPFLAGS_VALID_VERSION_TAG;
            ty.tp_version_tag = 0;
        }
        let mut left_bases = raw_tuple(&[&raw mut *root]);
        let mut right_bases = raw_tuple(&[&raw mut *root]);
        let mut diamond_bases = raw_tuple(&[&raw mut *left, &raw mut *right]);
        left.tp_bases = (&raw mut left_bases).cast();
        right.tp_bases = (&raw mut right_bases).cast();
        diamond.tp_bases = (&raw mut diamond_bases).cast();
        assert!(unsafe { assign_type_version_tag(&raw mut *diamond, &mut HashSet::new()) });
        assert!(
            [&root, &left, &right, &diamond]
                .into_iter()
                .all(|ty| ty.tp_flags & Py_TPFLAGS_VALID_VERSION_TAG != 0)
        );

        let mut cycle_a = blank_type(1);
        let mut cycle_b = blank_type(1);
        cycle_a.tp_flags &= !Py_TPFLAGS_VALID_VERSION_TAG;
        cycle_b.tp_flags &= !Py_TPFLAGS_VALID_VERSION_TAG;
        let mut a_bases = raw_tuple(&[&raw mut *cycle_b]);
        let mut b_bases = raw_tuple(&[&raw mut *cycle_a]);
        cycle_a.tp_bases = (&raw mut a_bases).cast();
        cycle_b.tp_bases = (&raw mut b_bases).cast();
        assert!(!unsafe { assign_type_version_tag(&raw mut *cycle_a, &mut HashSet::new()) });

        let local_counter = AtomicU32::new(u32::MAX);
        assert_eq!(allocate_type_version_tag(&local_counter), None);
        assert_eq!(local_counter.load(Ordering::SeqCst), u32::MAX);
    }

    #[test]
    fn wide_subclass_teardown_is_linear_and_returns_registry_to_baseline() {
        const WIDTH: usize = 8192;
        let mut base = blank_type(1);
        let base_address = (&raw mut *base).addr();
        let mut children: Vec<_> = (0..WIDTH).map(|_| blank_type(1)).collect();
        let addresses: Vec<_> = children
            .iter_mut()
            .map(|child| (&raw mut **child).addr())
            .collect();
        for address in &addresses {
            unsafe {
                register_subclass(
                    ptr::with_exposed_provenance_mut(base_address),
                    ptr::with_exposed_provenance_mut(*address),
                )
            };
        }
        let started = std::time::Instant::now();
        for address in &addresses {
            unregister_type_address(*address);
        }
        let teardown = started.elapsed();
        assert!(
            teardown < std::time::Duration::from_secs(5),
            "8192-wide teardown exceeded linear receipt: {teardown:?}"
        );
        let registry = TYPE_SUBCLASSES.lock();
        assert!(
            addresses
                .iter()
                .all(|address| !registry.live.contains_key(address))
        );
        drop(registry);
        unregister_type_address(base_address);
    }
}
