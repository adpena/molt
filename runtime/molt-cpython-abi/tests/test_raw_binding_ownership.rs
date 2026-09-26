//! Raw bridge identities are borrowed from static or foreign lifetime
//! authorities and never release a runtime reference. Runtime-backed ABI
//! callables are canonical managed views (see test_cfunction_bridge_registration).
mod support;

use molt_cpython_abi::abi_types::{PyObject, PyTypeObject};
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
