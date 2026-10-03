//! Physical publication tests: exercise the real maps, projection ledger and
//! retirement of a type/MRO cycle in both construction orders.

use super::*;

fn install(
    bridge: &ObjectBridge,
    bits: AbiHandle,
    is_type: bool,
) -> (*mut PyObject, PublicationBuildGuard<'_>) {
    let guard = PublicationBuildGuard::enter(bridge, bits).unwrap();
    let view = if is_type {
        let name = std::ffi::CString::new("PublicationCycle").unwrap();
        let mut object: PyTypeObject = unsafe { std::mem::zeroed() };
        // One caller reference and the transaction pin.
        object.ob_base.ob_base.ob_refcnt = 2;
        object.ob_base.ob_base.ob_type = &raw mut PyType_Type;
        object.tp_name = name.as_ptr();
        ManagedView::Type {
            object: ManagedTypeAllocation::new(object),
            _name: name,
        }
    } else {
        ManagedView::Tuple {
            allocation: TupleAllocation::new(2, &raw mut PyTuple_Type, 1).unwrap(),
        }
    };
    let pointer = view.py_obj();
    let entry = Box::new(BridgeEntry {
        view,
        bits,
        unicode: None,
        publication: PublicationState::Building {
            owner: std::thread::current().id(),
        },
        lifecycle: BridgeLifecycle::ViewHoldOnly,
    });
    bridge
        .insert_managed_entry(bits, entry)
        .map_err(|(_, error)| error)
        .unwrap();
    guard.inserted(pointer);
    (pointer, guard)
}

unsafe fn retain_edge(bridge: &ObjectBridge, bits: AbiHandle) -> *mut PyObject {
    // The lookup both acquires the reference and observes its dependency.
    let pointer = unsafe { bridge.published_pyobj(bits, true) }.unwrap();
    assert!(!pointer.is_null());
    assert!(unsafe { bridge.projection_adopt_owned_ref(pointer) });
    pointer
}

fn cycle(type_first: bool, commit: bool) {
    init_tag_table();
    let bridge = ObjectBridge::new();
    let type_bits = MoltObject::from_int(71_001).bits();
    let tuple_bits = MoltObject::from_int(71_002).bits();
    let (first_bits, second_bits) = if type_first {
        (type_bits, tuple_bits)
    } else {
        (tuple_bits, type_bits)
    };
    let (first, first_guard) = install(&bridge, first_bits, type_first);
    let (second, second_guard) = install(&bridge, second_bits, !type_first);
    let (type_ptr, tuple_ptr) = if type_first {
        (first.cast::<PyTypeObject>(), second)
    } else {
        (second.cast::<PyTypeObject>(), first)
    };
    unsafe {
        // Complete the child's back-edge before returning its provisional view.
        if type_first {
            (*tuple_ptr.cast::<crate::abi_types::PyTupleObject>()).ob_item[0] =
                retain_edge(&bridge, type_bits);
        } else {
            (*type_ptr).tp_mro = retain_edge(&bridge, tuple_bits);
        }
    }
    assert!(second_guard.finish());
    assert!(matches!(
        bridge.handle_shard(second_bits).lock().to_py[&second_bits].publication,
        PublicationState::Building { .. }
    ));
    unsafe {
        if type_first {
            (*type_ptr).tp_mro = retain_edge(&bridge, tuple_bits);
        } else {
            (*tuple_ptr.cast::<crate::abi_types::PyTupleObject>()).ob_item[0] =
                retain_edge(&bridge, type_bits);
        }
    }
    assert_eq!(bridge.mirrored_c_refcount(first.addr()), 1);
    assert_eq!(bridge.mirrored_c_refcount(second.addr()), 1);
    if commit {
        assert!(first_guard.finish());
        for bits in [first_bits, second_bits] {
            assert_eq!(
                bridge.handle_shard(bits).lock().to_py[&bits].publication,
                PublicationState::Ready
            );
        }
        // Keep a direct alias of the MRO and an unrelated live projection.
        // Reverse closure must retire the alias, but preserve the unrelated
        // component. No test code manually breaks the physical cycle.
        let alias_bits = MoltObject::from_int(71_003).bits();
        let (alias, alias_guard) = install(&bridge, alias_bits, false);
        unsafe {
            (*alias.cast::<crate::abi_types::PyTupleObject>()).ob_item[0] =
                retain_edge(&bridge, tuple_bits);
        }
        assert!(alias_guard.finish());
        let unrelated_bits = MoltObject::from_int(71_004).bits();
        let (unrelated, unrelated_guard) = install(&bridge, unrelated_bits, false);
        assert!(unrelated_guard.finish());
        assert_eq!(bridge.mirrored_c_refcount(tuple_ptr.addr()), 2);
        assert!(bridge.retire_runtime_type_views(&[type_bits]));
        // The shared transaction asserts zero incoming mirrors while every
        // C allocation is still live, before map revocation and physical free.
        for pointer in [type_ptr.cast(), tuple_ptr, alias] {
            assert!(bridge.managed_handle_for_pyobj(pointer).is_none());
            assert_eq!(bridge.mirrored_c_refcount(pointer.addr()), 0);
        }
        assert_eq!(
            bridge.managed_handle_for_pyobj(unrelated),
            Some(unrelated_bits)
        );
        assert_eq!(
            bridge.release_pyobj(unrelated),
            PyObjRelease::ManagedViewRetired
        );
    } else {
        // Simulate failure in a later publication stage (dictionary or retain).
        drop(first_guard);
    }
    for (bits, pointer) in [(first_bits, first), (second_bits, second)] {
        assert!(!bridge.handle_shard(bits).lock().to_py.contains_key(&bits));
        assert!(
            !bridge
                .address_shard(pointer.addr())
                .lock()
                .from_py
                .contains_key(&pointer.addr())
        );
        assert_eq!(bridge.mirrored_c_refcount(pointer.addr()), 0);
    }
    PUBLICATION_BUILD_STACK.with(|stack| {
        let stack = stack.borrow();
        assert!(stack.frames.is_empty() && stack.active.is_empty());
    });
}

#[test]
fn recursive_type_then_mro_rolls_back_without_dangling_projection() {
    cycle(true, false);
}

#[test]
fn recursive_mro_then_type_rolls_back_without_dangling_projection() {
    cycle(false, false);
}

#[test]
fn recursive_type_then_mro_commits_together() {
    cycle(true, true);
}

#[test]
fn recursive_mro_then_type_commits_together() {
    cycle(false, true);
}

#[test]
fn independent_completed_view_survives_outer_failure() {
    init_tag_table();
    let bridge = ObjectBridge::new();
    let outer_bits = MoltObject::from_int(72_001).bits();
    let inner_bits = MoltObject::from_int(72_002).bits();
    let (_, outer_guard) = install(&bridge, outer_bits, true);
    let (inner, inner_guard) = install(&bridge, inner_bits, false);
    assert!(inner_guard.finish());
    assert_eq!(
        bridge.handle_shard(inner_bits).lock().to_py[&inner_bits].publication,
        PublicationState::Ready
    );
    drop(outer_guard);
    assert_eq!(
        unsafe { bridge.published_pyobj(inner_bits, false) },
        Some(inner)
    );
    assert_eq!(unsafe { (*inner).ob_refcnt }, 1);
    assert_eq!(
        bridge.release_pyobj(inner),
        PyObjRelease::ManagedViewRetired
    );
}
