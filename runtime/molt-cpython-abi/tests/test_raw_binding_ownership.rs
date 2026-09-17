//! Raw bridge identities have explicit borrowed versus transferred custody.
//! The release hook reenters the same address shard and injects a cleanup error:
//! retirement must detach first, unlock, release once, and restore exact error.
mod support;

use molt_cpython_abi::abi_types::{PyExc_KeyError, PyExc_ValueError, PyObject, PyTypeObject};
use molt_cpython_abi::api::{errors, refcount};
use molt_cpython_abi::bridge::{GLOBAL_BRIDGE, StaticBindingError};
use molt_lang_obj_model::MoltObject;
use std::ptr;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

static LOCK: Mutex<()> = Mutex::new(());
static RELEASES: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static WATCH: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static REENTER_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static REENTRY_SAW_DETACHED: AtomicBool = AtomicBool::new(false);
static CLEANUP_ERROR: AtomicBool = AtomicBool::new(false);
static DEALLOCS: AtomicUsize = AtomicUsize::new(0);

fn bits(index: usize) -> u64 {
    MoltObject::from_ptr((0x6a00_0000usize + index * 0x10) as *mut u8).bits()
}

unsafe extern "C" fn release_runtime(bits: u64) {
    if !WATCH.lock().unwrap().contains(&bits) {
        return;
    }
    RELEASES.lock().unwrap().push(bits);
    let addr = REENTER_ADDRESS.load(Ordering::Relaxed);
    if addr != 0 {
        let object = ptr::with_exposed_provenance_mut::<PyObject>(addr);
        REENTRY_SAW_DETACHED.store(
            GLOBAL_BRIDGE.molt_handle_for_pyobj(object).is_none(),
            Ordering::Relaxed,
        );
    }
    if CLEANUP_ERROR.load(Ordering::Relaxed) {
        unsafe {
            errors::PyErr_SetString(
                (&raw mut PyExc_KeyError).cast(),
                c"cleanup callback".as_ptr(),
            )
        };
    }
}

unsafe extern "C" fn deallocate(object: *mut PyObject) {
    DEALLOCS.fetch_add(1, Ordering::Relaxed);
    if REENTER_ADDRESS.load(Ordering::Relaxed) == object.addr() {
        REENTRY_SAW_DETACHED.store(
            GLOBAL_BRIDGE.molt_handle_for_pyobj(object).is_none(),
            Ordering::Relaxed,
        );
    }
    unsafe { drop(Box::from_raw(object)) };
}

fn setup(watch: &[u64]) {
    let mut hooks = support::stub_runtime_hooks();
    support::fake_strings::wire(&mut hooks);
    hooks.dec_ref = release_runtime;
    hooks.foreign_new = support::fake_foreign::foreign_new;
    support::prepare_abi_test_thread(hooks);
    *WATCH.lock().unwrap() = watch.to_vec();
    RELEASES.lock().unwrap().clear();
    REENTER_ADDRESS.store(0, Ordering::Relaxed);
    REENTRY_SAW_DETACHED.store(false, Ordering::Relaxed);
    CLEANUP_ERROR.store(false, Ordering::Relaxed);
    DEALLOCS.store(0, Ordering::Relaxed);
    unsafe { errors::PyErr_Clear() };
}

fn raw_object(typ: &mut PyTypeObject) -> *mut PyObject {
    typ.tp_dealloc = Some(deallocate);
    Box::into_raw(Box::new(PyObject {
        ob_refcnt: 1,
        ob_type: typ,
    }))
}

#[test]
fn owned_raw_binding_releases_once_after_detachment_before_exact_type_deallocation() {
    let _lock = LOCK.lock().unwrap();
    setup(&[bits(1)]);
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let object = raw_object(&mut typ);
    unsafe {
        assert!(GLOBAL_BRIDGE.register_pyobj_for_handle(object, bits(1)));
        assert_eq!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(object)
                .map(|value| value.bits()),
            Some(bits(1))
        );
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits(1)), object);
        refcount::Py_INCREF(object);
        refcount::Py_DECREF(object);
        assert!(RELEASES.lock().unwrap().is_empty());
        REENTER_ADDRESS.store(object.expose_provenance(), Ordering::Relaxed);
        CLEANUP_ERROR.store(true, Ordering::Relaxed);
        errors::PyErr_SetString(
            (&raw mut PyExc_ValueError).cast(),
            c"original exception".as_ptr(),
        );
        let error = errors::take_current_error().unwrap();
        let original_value = error.value;
        errors::restore_current_error_exact(error);
        refcount::Py_DECREF(object);
        assert_eq!(*RELEASES.lock().unwrap(), [bits(1)]);
        assert_eq!(DEALLOCS.load(Ordering::Relaxed), 1);
        assert!(REENTRY_SAW_DETACHED.load(Ordering::Relaxed));
        let error = errors::take_current_error().unwrap();
        assert_eq!(error.exc_type, (&raw mut PyExc_ValueError).cast());
        assert_eq!(error.value, original_value);
        drop(error);
    }
    REENTER_ADDRESS.store(0, Ordering::Relaxed);
}

#[test]
fn owned_raw_binding_success_suppresses_cleanup_only_error() {
    let _lock = LOCK.lock().unwrap();
    setup(&[bits(2)]);
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let object = raw_object(&mut typ);
    unsafe {
        assert!(GLOBAL_BRIDGE.register_pyobj_for_handle(object, bits(2)));
        CLEANUP_ERROR.store(true, Ordering::Relaxed);
        refcount::Py_DECREF(object);
        assert!(errors::PyErr_Occurred().is_null());
    }
    assert_eq!(*RELEASES.lock().unwrap(), [bits(2)]);
    assert_eq!(DEALLOCS.load(Ordering::Relaxed), 1);
}

#[test]
fn raw_owned_registration_collision_consumes_only_rejected_hold() {
    let _lock = LOCK.lock().unwrap();
    setup(&[bits(3), bits(4)]);
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let first = raw_object(&mut typ);
    let second = raw_object(&mut typ);
    unsafe {
        assert!(GLOBAL_BRIDGE.register_pyobj_for_handle(first, bits(3)));
        // Each attempted registration transfers its own input reference, even
        // when the handle is the same identity as a prior successful call.
        assert!(!GLOBAL_BRIDGE.register_pyobj_for_handle(second, bits(3)));
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        assert_eq!(*RELEASES.lock().unwrap(), [bits(3)]);
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits(3)), first);
        assert!(GLOBAL_BRIDGE.molt_handle_for_pyobj(second).is_none());
        assert!(!GLOBAL_BRIDGE.register_pyobj_for_handle(first, bits(4)));
        assert!(!errors::PyErr_Occurred().is_null());
        errors::PyErr_Clear();
        assert_eq!(*RELEASES.lock().unwrap(), [bits(3), bits(4)]);
        assert_eq!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(first)
                .map(|value| value.bits()),
            Some(bits(3))
        );
        refcount::Py_DECREF(second);
        refcount::Py_DECREF(first);
    }
    assert_eq!(*RELEASES.lock().unwrap(), [bits(3), bits(4), bits(3)]);
    assert_eq!(DEALLOCS.load(Ordering::Relaxed), 2);
}

#[test]
fn borrowed_static_operations_cannot_erase_or_replace_owned_custody() {
    let _lock = LOCK.lock().unwrap();
    setup(&[bits(5), bits(6)]);
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let owned = raw_object(&mut typ);
    let alias = raw_object(&mut typ);
    unsafe {
        assert!(GLOBAL_BRIDGE.register_pyobj_for_handle(owned, bits(5)));
        assert!(!GLOBAL_BRIDGE.unbind_static_pyobj_from_runtime_handle(owned, bits(5)));
        assert_eq!(
            GLOBAL_BRIDGE.bind_static_pyobj_to_runtime_handle(owned, bits(6), true),
            Err(StaticBindingError::OwnedPrevious {
                bits: bits(5),
                address: owned.addr()
            })
        );
        assert_eq!(
            GLOBAL_BRIDGE.bind_static_pyobj_to_runtime_handle(alias, bits(5), false),
            Err(StaticBindingError::OwnedTarget {
                address: owned.addr()
            })
        );
        assert!(RELEASES.lock().unwrap().is_empty());
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits(5)), owned);
        refcount::Py_DECREF(alias);
        refcount::Py_DECREF(owned);
    }
    assert_eq!(*RELEASES.lock().unwrap(), [bits(5)]);
}

#[test]
fn borrowed_static_alias_retirement_preserves_reverse_identity_and_runtime_owner() {
    let _lock = LOCK.lock().unwrap();
    setup(&[bits(7)]);
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let canonical = raw_object(&mut typ);
    let alias = raw_object(&mut typ);
    unsafe {
        assert!(
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(canonical, bits(7), true)
                .is_ok()
        );
        assert!(
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(alias, bits(7), false)
                .is_ok()
        );
        refcount::Py_DECREF(alias);
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits(7)), canonical);
        assert!(RELEASES.lock().unwrap().is_empty());
        refcount::Py_DECREF(canonical);
    }
    assert!(RELEASES.lock().unwrap().is_empty());
    assert_eq!(DEALLOCS.load(Ordering::Relaxed), 2);
}

#[test]
fn borrowed_static_without_reverse_detaches_before_native_deallocation() {
    let _lock = LOCK.lock().unwrap();
    setup(&[bits(12)]);
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let alias = raw_object(&mut typ);
    unsafe {
        assert!(
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(alias, bits(12), false)
                .is_ok()
        );
        REENTER_ADDRESS.store(alias.expose_provenance(), Ordering::Relaxed);
        refcount::Py_DECREF(alias);
    }
    assert!(REENTRY_SAW_DETACHED.load(Ordering::Relaxed));
    assert_eq!(DEALLOCS.load(Ordering::Relaxed), 1);
    assert!(RELEASES.lock().unwrap().is_empty());
    REENTER_ADDRESS.store(0, Ordering::Relaxed);
}

#[test]
fn borrowed_static_rebinding_preserves_aliases_and_rejects_collision_without_mutation() {
    let _lock = LOCK.lock().unwrap();
    setup(&[bits(8), bits(9), bits(10)]);
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let canonical = raw_object(&mut typ);
    let alias = raw_object(&mut typ);
    let occupied = raw_object(&mut typ);
    unsafe {
        assert!(
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(canonical, bits(8), true)
                .is_ok()
        );
        assert!(
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(alias, bits(8), false)
                .is_ok()
        );
        assert!(
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(occupied, bits(9), true)
                .is_ok()
        );
        assert_eq!(
            GLOBAL_BRIDGE.bind_static_pyobj_to_runtime_handle(canonical, bits(9), true),
            Err(StaticBindingError::CanonicalTargetBorrowed {
                address: occupied.addr()
            })
        );
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits(8)), canonical);
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits(9)), occupied);
        assert_eq!(
            GLOBAL_BRIDGE
                .molt_handle_for_pyobj(canonical)
                .map(|value| value.bits()),
            Some(bits(8))
        );
        assert!(
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(alias, bits(10), true)
                .is_ok()
        );
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits(8)), canonical);
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits(10)), alias);
        assert!(GLOBAL_BRIDGE.unbind_static_pyobj_from_runtime_handle(alias, bits(10)));
        assert!(
            GLOBAL_BRIDGE
                .bind_static_pyobj_to_runtime_handle(canonical, bits(10), true)
                .is_ok()
        );
        assert!(!GLOBAL_BRIDGE.unbind_static_pyobj_from_runtime_handle(canonical, bits(8)));
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits(10)), canonical);
        refcount::Py_DECREF(alias);
        refcount::Py_DECREF(canonical);
        refcount::Py_DECREF(occupied);
    }
    assert!(RELEASES.lock().unwrap().is_empty());
    assert_eq!(DEALLOCS.load(Ordering::Relaxed), 3);
}

#[test]
fn borrowed_foreign_wrapper_reverse_entry_never_releases_its_own_runtime_identity() {
    let _lock = LOCK.lock().unwrap();
    setup(&[]);
    let mut typ: PyTypeObject = unsafe { std::mem::zeroed() };
    let foreign = raw_object(&mut typ);
    unsafe {
        let wrapper = GLOBAL_BRIDGE
            .molt_value_for_pyobj(foreign)
            .expect("genuine foreign crossing returns one owned runtime wrapper");
        WATCH.lock().unwrap().push(wrapper);
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(wrapper), foreign);
        assert_eq!((*foreign).ob_refcnt, 2);
        assert_eq!(
            GLOBAL_BRIDGE.bind_static_pyobj_to_runtime_handle(foreign, bits(11), true),
            Err(StaticBindingError::AddressIdentityConflict {
                forward: None,
                direct: None,
                foreign: Some(wrapper),
                foreign_inflight: false,
                numeric_carrier: None,
            })
        );
        assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(wrapper), foreign);
        // The runtime wrapper's own destruction calls release_foreign, then
        // retires its separate C edge; reverse lookup owns no extra wrapper ref.
        GLOBAL_BRIDGE.release_foreign(foreign.addr());
        assert!(RELEASES.lock().unwrap().is_empty());
        refcount::Py_DECREF(foreign);
        refcount::Py_DECREF(foreign);
    }
    assert_eq!(DEALLOCS.load(Ordering::Relaxed), 1);
    assert!(RELEASES.lock().unwrap().is_empty());
}
