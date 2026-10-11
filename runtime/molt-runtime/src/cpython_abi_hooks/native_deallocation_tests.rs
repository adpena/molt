//! Native destruction exercises public ownership transitions and real GC state.

use super::native_test_fixture::NativeType;
use crate::concurrency::gil::with_gil;
use molt_cpython_abi::abi_types::*;
use molt_cpython_abi::api::{errors, memory, object, refcount, sequences, typeobj};
use refcount::OwnedPyObject;
use std::ffi::c_void;
use std::ptr;

unsafe fn prior_error() -> OwnedPyObject {
    unsafe {
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"native retirement".as_ptr(),
        );
        OwnedPyObject::from_owned(errors::PyErr_GetRaisedException())
    }
}

unsafe fn observed_error() -> *mut PyObject {
    unsafe {
        let pending = errors::PyErr_GetRaisedException();
        errors::PyErr_SetRaisedException(pending);
        pending
    }
}

unsafe fn native_container(
    type_: *mut PyTypeObject,
    tuple: bool,
    child: OwnedPyObject,
) -> OwnedPyObject {
    unsafe {
        let outer =
            OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(type_, isize::from(tuple)));
        assert!(!outer.as_ptr().is_null());
        if tuple {
            // The extension writes its own initialized inline item. This never
            // calls the managed PyTuple_New constructor or creates a Rust Vec.
            let items = ptr::addr_of_mut!((*outer.as_ptr().cast::<PyTupleObject>()).ob_item)
                .cast::<*mut PyObject>();
            *items = child.into_ptr();
        } else {
            assert_eq!(sequences::PyList_Append(outer.as_ptr(), child.as_ptr()), 0);
        }
        outer
    }
}

#[test]
fn native_deallocation_deep_list_tuple_and_heap_subtype_chains() {
    let transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        for minor in [12, 13, 14] {
            transaction.with_target_python_minor(&py, minor, || {
                let mut slots = [PyType_Slot {
                    slot: 0,
                    pfunc: ptr::null_mut(),
                }];
                let mut spec = PyType_Spec {
                    name: c"deallocation.DeepList".as_ptr(),
                    basicsize: 0,
                    itemsize: 0,
                    flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
                    slots: slots.as_mut_ptr(),
                };
                let subclass = OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
                    &raw mut spec,
                    (&raw mut PyList_Type).cast(),
                ));
                assert!(!subclass.as_ptr().is_null());
                let type_refs = (*subclass.as_ptr()).ob_refcnt;
                let prior = prior_error();
                for (type_, tuple) in [
                    (&raw mut PyList_Type, false),
                    (&raw mut PyTuple_Type, true),
                    (subclass.as_ptr().cast(), false),
                ] {
                    for macro_entry in [false, true] {
                        let leaf = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                            &raw mut PyBaseObject_Type,
                            0,
                        ));
                        let mut root = OwnedPyObject::from_borrowed(leaf.as_ptr());
                        let mut addresses = Vec::with_capacity(4096);
                        for _ in 0..4096 {
                            root = native_container(type_, tuple, root);
                            addresses.push(root.as_ptr().addr());
                        }
                        errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
                        if macro_entry {
                            let object = root.into_ptr();
                            assert_eq!((*object).ob_refcnt, 1);
                            (*object).ob_refcnt = 0;
                            refcount::_Py_Dealloc(object);
                        } else {
                            drop(root);
                        }
                        let restored =
                            OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                        assert_eq!(restored.as_ptr(), prior.as_ptr());
                        assert_eq!((*leaf.as_ptr()).ob_refcnt, 1);
                        assert!(
                            addresses
                                .into_iter()
                                .all(|address| !crate::object::gc::native_gc_is_enrolled(address))
                        );
                        assert_eq!((*subclass.as_ptr()).ob_refcnt, type_refs);
                    }
                }
                assert_eq!(typeobj::molt_type_clear(subclass.as_ptr()), 0);
            });
        }
        assert!(!crate::exception_pending(&py));
        assert!(errors::PyErr_Occurred().is_null());
    });
}

#[derive(Default)]
struct MixedWitness {
    active: usize,
    peak: usize,
    completed: usize,
    prior: *mut PyObject,
    bad_error: bool,
}

#[repr(C)]
struct ImmediateNode {
    header: PyObject,
    child: *mut PyObject,
    witness: *mut MixedWitness,
}

unsafe extern "C" fn immediate_dealloc(object: *mut PyObject) {
    unsafe {
        let node = object.cast::<ImmediateNode>();
        let witness = (*node).witness;
        (*witness).active += 1;
        (*witness).peak = (*witness).peak.max((*witness).active);
        let child = (*node).child;
        (*node).child = ptr::null_mut();
        refcount::Py_XDECREF(child);
        (*witness).bad_error |= observed_error() != (*witness).prior;
        (*witness).active -= 1;
        (*witness).completed += 1;
        memory::PyObject_Free(object.cast());
    }
}

#[test]
fn native_deallocation_interleaves_immediate_non_gc_nodes_without_deferring_them() {
    let transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        for minor in [12, 13, 14] {
            transaction.with_target_python_minor(&py, minor, || {
                let mut witness = MixedWitness::default();
                let mut class = NativeType::<PyTypeObject>::subtype(
                    &raw mut PyBaseObject_Type,
                    c"deallocation.Immediate",
                );
                class.tp_basicsize = std::mem::size_of::<ImmediateNode>() as isize;
                class.tp_dealloc = Some(immediate_dealloc);
                assert_eq!(class.ready(), 0);
                assert_eq!(class.tp_flags & Py_TPFLAGS_HAVE_GC, 0);
                let prior = prior_error();
                witness.prior = prior.as_ptr();
                let mut root = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                    &raw mut PyBaseObject_Type,
                    0,
                ));
                for _ in 0..2048 {
                    root = native_container(&raw mut PyList_Type, false, root);
                    let node =
                        typeobj::PyType_GenericAlloc(&raw mut *class, 0).cast::<ImmediateNode>();
                    assert!(!node.is_null());
                    (*node).witness = &raw mut witness;
                    (*node).child = root.into_ptr();
                    root = OwnedPyObject::from_owned(node.cast());
                }
                errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
                drop(root);
                let restored = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(restored.as_ptr(), prior.as_ptr());
                assert_eq!(witness.completed, 2048);
                assert_eq!(witness.active, 0);
                assert!(
                    witness.peak > 1 && witness.peak < 128,
                    "native recursion was not bounded: {}",
                    witness.peak
                );
                assert!(!witness.bad_error);
            });
        }
        assert!(!crate::exception_pending(&py));
    });
}

unsafe extern "C" fn custom_gc_traverse(
    object: *mut PyObject,
    visit: *mut c_void,
    context: *mut c_void,
) -> std::os::raw::c_int {
    unsafe {
        let child = (*object.cast::<ImmediateNode>()).child;
        if child.is_null() {
            return 0;
        }
        let visit: unsafe extern "C" fn(*mut PyObject, *mut c_void) -> std::os::raw::c_int =
            std::mem::transmute(visit);
        visit(child, context)
    }
}

unsafe extern "C" fn custom_gc_dealloc(object: *mut PyObject) {
    unsafe {
        let node = object.cast::<ImmediateNode>();
        let witness = (*node).witness;
        memory::PyObject_GC_UnTrack(object.cast());
        (*witness).active += 1;
        (*witness).peak = (*witness).peak.max((*witness).active);
        refcount::Py_CLEAR(&raw mut (*node).child);
        (*witness).bad_error |= observed_error() != (*witness).prior;
        (*witness).active -= 1;
        (*witness).completed += 1;
        memory::PyObject_GC_Del(object.cast());
    }
}

#[test]
fn native_deallocation_custom_gc_slot_participates_only_in_python_314() {
    let transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        for (minor, immediate) in [(12, true), (13, true), (14, false)] {
            transaction.with_target_python_minor(&py, minor, || {
                let mut witness = MixedWitness::default();
                let mut class = NativeType::<PyTypeObject>::subtype(
                    &raw mut PyBaseObject_Type,
                    c"deallocation.CustomGc",
                );
                class.tp_basicsize = std::mem::size_of::<ImmediateNode>() as isize;
                class.tp_flags |= Py_TPFLAGS_HAVE_GC;
                class.tp_dealloc = Some(custom_gc_dealloc);
                class.tp_traverse = Some(custom_gc_traverse);
                assert_eq!(class.ready(), 0);
                let prior = prior_error();
                witness.prior = prior.as_ptr();
                let mut root = OwnedPyObject::from_owned(ptr::null_mut());
                for _ in 0..256 {
                    let node =
                        typeobj::PyType_GenericAlloc(&raw mut *class, 0).cast::<ImmediateNode>();
                    assert!(!node.is_null());
                    (*node).witness = &raw mut witness;
                    (*node).child = root.into_ptr();
                    root = OwnedPyObject::from_owned(node.cast());
                }
                errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
                drop(root);
                let restored = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(restored.as_ptr(), prior.as_ptr());
                assert_eq!(witness.completed, 256);
                assert_eq!(witness.active, 0);
                if immediate {
                    assert_eq!(witness.peak, 256, "legacy external slot was newly deferred");
                } else {
                    assert!(witness.peak > 1 && witness.peak < 128);
                }
                assert!(!witness.bad_error);
            });
        }
        assert!(!crate::exception_pending(&py));
    });
}

struct ZeroReferenceOwner {
    object: *mut PyObject,
    queued: bool,
}

impl Drop for ZeroReferenceOwner {
    fn drop(&mut self) {
        unsafe {
            if self.queued {
                crate::object::gc::native_gc_resume(self.object.addr());
            }
            refcount::_Py_Dealloc(self.object);
        }
    }
}

#[test]
fn native_deallocation_queue_retains_identity_and_restores_only_requested_tracking() {
    let _transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        for tracked in [false, true] {
            for restore in [false, true] {
                let object = typeobj::PyType_GenericAlloc(&raw mut PyList_Type, 0);
                assert!(!object.is_null());
                if !tracked {
                    memory::PyObject_GC_UnTrack(object.cast());
                }
                (*object).ob_refcnt = 0;
                let mut owner = ZeroReferenceOwner {
                    object,
                    queued: false,
                };
                let address = object.addr();
                assert!(crate::object::gc::native_gc_defer(address, 0, restore));
                owner.queued = true;
                assert!(crate::object::gc::native_gc_is_enrolled(address));
                assert!(!crate::object::gc::native_gc_is_tracked(address));
                assert!(!crate::object::gc::native_gc_is_finalized(address));
                assert!(!crate::object::gc::native_gc_defer(address, 0, restore));
                assert!(!crate::object::gc::native_gc_track(address));
                assert_eq!(
                    crate::object::gc::collect_cycles(&py).status,
                    crate::object::gc::GcCollectStatus::Completed
                );
                assert!(crate::object::gc::native_gc_is_enrolled(address));
                assert_eq!(crate::object::gc::native_gc_resume(address), 0);
                owner.queued = false;
                assert_eq!(
                    crate::object::gc::native_gc_is_tracked(address),
                    tracked && restore
                );
                drop(owner);
                assert!(!crate::object::gc::native_gc_is_enrolled(address));
            }
        }
        assert!(!crate::exception_pending(&py));
    });
}

#[derive(Default)]
struct ResurrectionWitness {
    selected: *mut PyObject,
    retained: *mut PyObject,
    prior: *mut PyObject,
    finalizers: usize,
    saw_queued_untracked: bool,
    saw_queued_unfinalized: bool,
    bad_error: bool,
    select: bool,
}

#[repr(C)]
struct ObservedList {
    list: PyListObject,
    witness: *mut ResurrectionWitness,
}

#[repr(C)]
struct QueueObserver {
    header: PyObject,
    child_address: usize,
    witness: *mut ResurrectionWitness,
}

unsafe extern "C" fn queue_observer_dealloc(object: *mut PyObject) {
    unsafe {
        let observer = object.cast::<QueueObserver>();
        let witness = (*observer).witness;
        let address = (*observer).child_address;
        if (*witness).select
            && (*witness).selected.is_null()
            && crate::object::gc::native_gc_is_enrolled(address)
        {
            let child = ptr::with_exposed_provenance_mut::<PyObject>(address);
            if (*child).ob_refcnt == 0 {
                (*witness).selected = child;
                (*witness).saw_queued_untracked = !crate::object::gc::native_gc_is_tracked(address);
                (*witness).saw_queued_unfinalized =
                    !crate::object::gc::native_gc_is_finalized(address);
            }
        }
        (*witness).bad_error |= observed_error() != (*witness).prior;
        memory::PyObject_Free(object.cast());
    }
}

unsafe extern "C" fn list_finalizer(object: *mut PyObject) {
    unsafe {
        let witness = (*object.cast::<ObservedList>()).witness;
        (*witness).finalizers += 1;
        // tp_finalize has its existing explicit saved-error boundary. The
        // ordinary observer destructor below still sees the exact outer error.
        (*witness).bad_error |= !observed_error().is_null();
        if (*witness).select && (*witness).selected == object {
            refcount::Py_INCREF(object);
            (*witness).retained = object;
            (*witness).select = false;
        }
    }
}

struct ResurrectionCleanup(*mut ResurrectionWitness);
impl Drop for ResurrectionCleanup {
    fn drop(&mut self) {
        unsafe {
            (*self.0).select = false;
            let retained = std::mem::replace(&mut (*self.0).retained, ptr::null_mut());
            refcount::Py_XDECREF(retained);
        }
    }
}

#[test]
fn native_deallocation_resurrects_a_deferred_heap_list_before_releasing_its_payload() {
    let transaction = crate::test_support::RuntimeTestTransaction::with_gc_isolation();
    with_gil(|py| unsafe {
        for minor in [12, 13, 14] {
            transaction.with_target_python_minor(&py, minor, || {
                let mut witness = ResurrectionWitness::default();
                let mut observer_class = NativeType::<PyTypeObject>::subtype(
                    &raw mut PyBaseObject_Type,
                    c"deallocation.QueueObserver",
                );
                observer_class.tp_basicsize = std::mem::size_of::<QueueObserver>() as isize;
                observer_class.tp_dealloc = Some(queue_observer_dealloc);
                assert_eq!(observer_class.ready(), 0);
                let mut slots = [
                    PyType_Slot {
                        slot: molt_cpython_abi::type_slots::Py_tp_finalize,
                        pfunc: list_finalizer as *const () as *mut c_void,
                    },
                    PyType_Slot {
                        slot: 0,
                        pfunc: ptr::null_mut(),
                    },
                ];
                let mut spec = PyType_Spec {
                    name: c"deallocation.ResurrectingList".as_ptr(),
                    basicsize: std::mem::size_of::<ObservedList>() as i32,
                    itemsize: 0,
                    flags: PyType_Spec::flags_from_tp_flags(Py_TPFLAGS_DEFAULT),
                    slots: slots.as_mut_ptr(),
                };
                let class = OwnedPyObject::from_owned(typeobj::PyType_FromSpecWithBases(
                    &raw mut spec,
                    (&raw mut PyList_Type).cast(),
                ));
                assert!(!class.as_ptr().is_null());
                let type_refs = (*class.as_ptr()).ob_refcnt;
                let _cleanup = ResurrectionCleanup(&raw mut witness);
                let prior = prior_error();
                witness.prior = prior.as_ptr();
                let mut root = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                    &raw mut PyBaseObject_Type,
                    0,
                ));
                let mut addresses = Vec::with_capacity(2048);
                for _ in 0..2048 {
                    let observer = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                        &raw mut *observer_class,
                        0,
                    ));
                    assert!(!observer.as_ptr().is_null());
                    (*observer.as_ptr().cast::<QueueObserver>()).witness = &raw mut witness;
                    (*observer.as_ptr().cast::<QueueObserver>()).child_address =
                        root.as_ptr().addr();
                    let outer = OwnedPyObject::from_owned(typeobj::PyType_GenericAlloc(
                        class.as_ptr().cast(),
                        0,
                    ));
                    assert!(!outer.as_ptr().is_null());
                    (*outer.as_ptr().cast::<ObservedList>()).witness = &raw mut witness;
                    // Terminal list destruction releases index 1 before index 0.
                    // The observer can therefore see a deferred child while its
                    // original parent unwinds, without knowing the queue limit.
                    assert_eq!(
                        sequences::PyList_Append(outer.as_ptr(), observer.as_ptr()),
                        0
                    );
                    assert_eq!(sequences::PyList_Append(outer.as_ptr(), root.as_ptr()), 0);
                    root = outer;
                    addresses.push(root.as_ptr().addr());
                }
                errors::PyErr_SetRaisedException(object::Py_NewRef(prior.as_ptr()));
                witness.select = true;
                drop(root);
                let retained = OwnedPyObject::from_owned(std::mem::replace(
                    &mut witness.retained,
                    ptr::null_mut(),
                ));
                assert!(
                    !retained.as_ptr().is_null(),
                    "no deferred object reached resurrection"
                );
                assert_eq!(retained.as_ptr(), witness.selected);
                assert!(witness.saw_queued_untracked && witness.saw_queued_unfinalized);
                assert_eq!(sequences::PyList_Size(retained.as_ptr()), 2);
                assert_ne!(memory::PyObject_GC_IsTracked(retained.as_ptr()), 0);
                assert_ne!(memory::PyObject_GC_IsFinalized(retained.as_ptr()), 0);
                drop(retained);
                assert_eq!(
                    witness.finalizers, 2048,
                    "deferred finalization replayed or was skipped"
                );
                assert!(!witness.bad_error);
                let restored = OwnedPyObject::from_owned(errors::PyErr_GetRaisedException());
                assert_eq!(restored.as_ptr(), prior.as_ptr());
                assert!(
                    addresses
                        .into_iter()
                        .all(|address| !crate::object::gc::native_gc_is_enrolled(address))
                );
                assert_eq!((*class.as_ptr()).ob_refcnt, type_refs);
                assert_eq!(typeobj::molt_type_clear(class.as_ptr()), 0);
            });
        }
        assert!(!crate::exception_pending(&py));
        assert!(errors::PyErr_Occurred().is_null());
    });
}
