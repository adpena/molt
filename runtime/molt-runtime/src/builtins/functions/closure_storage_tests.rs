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

#[test]
fn closure_store_self_assignment_preserves_sole_slot_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let owner = callable(_py, "closure_payload", closure_payload);
        let weak = watch(_py, owner, MoltObject::none().bits());
        // Transfer the allocation's sole reference into the closure slot.
        let mut slot = owner;
        let address = crate::provenance::abi::expose_address(&mut slot);
        unsafe { molt_closure_store(address, 0, owner) };
        assert_eq!(slot, owner);
        let observed = crate::molt_weakref_call(weak);
        assert_eq!(
            observed, owner,
            "self assignment must not finalize the slot owner"
        );
        dec_ref_bits(_py, observed);
        unsafe { molt_closure_store(address, 0, MoltObject::none().bits()) };
        assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
        dec_ref_bits(_py, weak);
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
        let mut slot = owner;
        let address = crate::provenance::abi::expose_address(&mut slot);
        CALLBACK_SLOT.store(address, AtomicOrdering::SeqCst);
        CALLBACK_OBSERVED.store(0, AtomicOrdering::SeqCst);

        unsafe { molt_closure_store(address, 0, incoming) };

        assert_eq!(CALLBACK_OBSERVED.load(AtomicOrdering::SeqCst), incoming);
        assert_eq!(slot, MoltObject::from_int(77).bits());
        assert!(obj_from_bits(crate::molt_weakref_call(weak)).is_none());
        CALLBACK_SLOT.store(0, AtomicOrdering::SeqCst);
        dec_ref_bits(_py, incoming);
        dec_ref_bits(_py, weak);
        dec_ref_bits(_py, callback);
        assert!(!exception_pending(_py));
    });
}
