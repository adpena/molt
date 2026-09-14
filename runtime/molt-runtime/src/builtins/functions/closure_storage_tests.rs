use super::*;

static CALLBACK_SLOT: AtomicU64 = AtomicU64::new(0);
static CALLBACK_OBSERVED: AtomicU64 = AtomicU64::new(0);

extern "C" fn closure_payload(_value: u64) -> u64 {
    MoltObject::none().bits()
}

extern "C" fn closure_reentrant_release(_weak: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let address = CALLBACK_SLOT.load(AtomicOrdering::SeqCst);
        let slot = crate::provenance::abi::mut_ptr::<u64>(address)
            .expect("registered closure slot address");
        assert!(!slot.is_null());
        CALLBACK_OBSERVED.store(unsafe { *slot }, AtomicOrdering::SeqCst);
        unsafe { molt_closure_store(address, 0, MoltObject::from_int(77).bits()) }
    })
}

fn callable(_py: &PyToken<'_>, name: &str, target: extern "C" fn(u64) -> u64) -> u64 {
    let ptr = alloc_runtime_function_obj(_py, runtime_fn_addr(name, target as *const ()), 1);
    assert!(!ptr.is_null());
    MoltObject::from_ptr(ptr).bits()
}

fn watch(_py: &PyToken<'_>, target: u64, callback: u64) -> u64 {
    let class = crate::molt_weakref_reference_type();
    let weak = crate::molt_weakref_new(class, target, callback);
    dec_ref_bits(_py, class);
    assert!(!exception_pending(_py));
    assert!(!obj_from_bits(weak).is_none());
    weak
}

fn closure_slot(_py: &PyToken<'_>, initial: u64) -> (u64, *mut u64, u64) {
    let storage = crate::molt_alloc(std::mem::size_of::<u64>() as u64);
    let ptr = obj_from_bits(storage)
        .as_ptr()
        .expect("allocated closure payload");
    let slot = ptr.cast::<u64>();
    let address = crate::provenance::abi::expose_address(ptr);
    unsafe { molt_closure_store(address, 0, initial) };
    crate::molt_object_publish_initialized(storage);
    assert!(!exception_pending(_py));
    (storage, slot, address)
}

#[test]
fn closure_payload_terminal_release_and_cycle_clear_own_every_capture_word() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        for cycle_clear in [false, true] {
            let value = callable(_py, "closure_payload", closure_payload);
            let weak = watch(_py, value, MoltObject::none().bits());
            let storage = crate::molt_alloc(16);
            let ptr = obj_from_bits(storage).as_ptr().unwrap();
            let address = crate::provenance::abi::expose_address(ptr);
            unsafe {
                molt_closure_store(address, 0, value);
                molt_closure_store(address, 8, value);
            }
            crate::molt_object_publish_initialized(storage);
            let mut captured = Vec::new();
            unsafe {
                crate::object::heap_lifecycle::visit_owned_edges(_py, ptr, &mut |child| {
                    captured.push(child)
                });
            }
            assert_eq!(
                captured
                    .iter()
                    .filter(|&&child| child == obj_from_bits(value).as_ptr().unwrap())
                    .count(),
                2
            );
            dec_ref_bits(_py, value);
            if cycle_clear {
                unsafe { crate::object::heap_lifecycle::clear_cycle_edges(_py, ptr) };
                assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
                unsafe { crate::object::heap_lifecycle::clear_cycle_edges(_py, ptr) };
            }
            dec_ref_bits(_py, storage);
            assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
            dec_ref_bits(_py, weak);
            assert!(!exception_pending(_py));
        }
    });
}

#[test]
fn closure_store_self_assignment_preserves_sole_slot_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let owner = callable(_py, "closure_payload", closure_payload);
        let weak = watch(_py, owner, MoltObject::none().bits());
        let (storage, slot, address) = closure_slot(_py, owner);
        // Leave the real payload as the value's sole strong owner.
        dec_ref_bits(_py, owner);
        unsafe { molt_closure_store(address, 0, owner) };
        assert_eq!(unsafe { *slot }, owner);
        assert_ne!(
            unsafe {
                (*crate::object::header_from_obj_ptr(slot.cast())).load_metadata_flags()
                    & crate::object::HEADER_FLAG_HAS_PTRS
            },
            0
        );
        let observed = crate::molt_weakref_call(weak);
        assert_eq!(
            observed, owner,
            "self assignment must not finalize the slot owner"
        );
        dec_ref_bits(_py, observed);
        unsafe { molt_closure_store(address, 0, MoltObject::none().bits()) };
        assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
        dec_ref_bits(_py, weak);
        dec_ref_bits(_py, storage);
        assert!(!exception_pending(_py));
    });
}

#[test]
fn closure_store_callback_observes_publication_and_keeps_reentrant_replacement() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let owner = callable(_py, "closure_payload", closure_payload);
        let callback = callable(_py, "closure_reentrant_release", closure_reentrant_release);
        let weak = watch(_py, owner, callback);
        let incoming_ptr = alloc_string(_py, b"published closure value");
        assert!(!incoming_ptr.is_null());
        let incoming = MoltObject::from_ptr(incoming_ptr).bits();
        let (storage, slot, address) = closure_slot(_py, owner);
        dec_ref_bits(_py, owner);
        CALLBACK_SLOT.store(address, AtomicOrdering::SeqCst);
        CALLBACK_OBSERVED.store(0, AtomicOrdering::SeqCst);

        unsafe { molt_closure_store(address, 0, incoming) };

        assert_eq!(CALLBACK_OBSERVED.load(AtomicOrdering::SeqCst), incoming);
        assert_eq!(unsafe { *slot }, MoltObject::from_int(77).bits());
        assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
        CALLBACK_SLOT.store(0, AtomicOrdering::SeqCst);
        dec_ref_bits(_py, incoming);
        dec_ref_bits(_py, weak);
        dec_ref_bits(_py, callback);
        dec_ref_bits(_py, storage);
        assert!(!exception_pending(_py));
    });
}
