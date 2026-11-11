use super::{METHOD_CACHE_SLOT_COUNT, MethodCache, clear_method_cache, clear_runtime_names};
use crate::{MoltObject, NameId, RuntimeNames, alloc_string, runtime_state};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(any(feature = "stdlib_math", feature = "stdlib_serial"))]
#[test]
fn bridge_name_failure_is_distinct_from_an_unsupported_name() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        assert_eq!(
            super::intern_bridge_protocol_name(py, b"unsupported-name"),
            Ok(None)
        );
        crate::raise_exception::<u64>(
            py,
            crate::BuiltinExc::MemoryError,
            "original bridge allocation failure",
        );
        let original = crate::exception_last_bits_noinc(py);
        for key in [b"__float__".as_slice(), b"unsupported-name"] {
            assert_eq!(super::intern_bridge_protocol_name(py, key), Err(()));
        }
        assert_eq!(crate::exception_last_bits_noinc(py), original);
        let _ = crate::molt_exception_clear_attached_body(py);
    });
}

#[test]
fn method_cache_slots_are_manifest_complete_and_unique() {
    let cache = MethodCache::new();
    let slots = cache.slots();
    assert_eq!(slots.len(), METHOD_CACHE_SLOT_COUNT);

    let mut seen = HashSet::with_capacity(slots.len());
    for slot in slots {
        let addr = slot as *const AtomicU64 as usize;
        assert!(seen.insert(addr));
        assert_eq!(slot.load(Ordering::Acquire), 0);
    }
}

#[test]
fn runtime_name_slots_are_manifest_complete_and_unique() {
    let names = RuntimeNames::new();
    let slots = names.slots();
    assert_eq!(slots.len(), NameId::ALL.len());

    let mut seen = HashSet::with_capacity(slots.len());
    for slot in slots {
        let addr = slot as *const AtomicU64 as usize;
        assert!(seen.insert(addr));
        assert_eq!(slot.load(Ordering::Acquire), 0);
    }

    let typed_slots: HashSet<usize> = NameId::ALL
        .iter()
        .map(|name| name.slot(&names) as *const AtomicU64 as usize)
        .collect();
    assert_eq!(
        typed_slots, seen,
        "typed identities cover the physical slots"
    );
}

#[test]
fn clear_method_cache_releases_every_remaining_private_callback() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let state = runtime_state(_py);
        clear_method_cache(_py, state);
        for slot in state.method_cache.slots() {
            let ptr = alloc_string(_py, b"private callback cache sentinel");
            assert!(!ptr.is_null());
            slot.store(MoltObject::from_ptr(ptr).bits(), Ordering::Release);
        }
        clear_method_cache(_py, state);
        for slot in state.method_cache.slots() {
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }
    });
}

#[test]
fn clear_runtime_names_releases_every_manifest_slot() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            // An unused TLS keyword must not allocate an object at startup.
            assert_eq!(
                NameId::Certfile.slot(&state.names).load(Ordering::Acquire),
                0
            );
            let certificate = NameId::Certfile.bits(_py);
            assert_ne!(certificate, 0);
            assert_eq!(NameId::Certfile.bits(_py), certificate);
            assert_eq!(NameId::from_bits(_py, certificate), Some(NameId::Certfile));
            let external = crate::object::builders::alloc_string_nointern(_py, b"certfile");
            assert!(!external.is_null());
            let external = MoltObject::from_ptr(external).bits();
            assert_ne!(external, certificate);
            assert_eq!(NameId::from_bits(_py, external), Some(NameId::Certfile));
            {
                let index = state.names.index.lock().unwrap_or_else(|e| e.into_inner());
                assert!(index.iter().any(|&(bits, _)| bits == certificate));
                assert!(
                    index.iter().all(|&(bits, name)| {
                        bits == name.slot(&state.names).load(Ordering::Acquire)
                    }),
                    "every borrowed index key must have its canonical slot owner"
                );
            }
            crate::dec_ref_bits(_py, external);
            clear_runtime_names(_py, state);
            assert!(
                state
                    .names
                    .index
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_empty()
            );

            let spec_bits = crate::NameId::DunderSpec.bits(_py);
            let cache_bits = crate::NameId::Origin.bits(_py);
            assert_ne!(spec_bits, 0);
            assert_ne!(cache_bits, 0);
            assert_ne!(spec_bits, cache_bits);

            clear_runtime_names(_py, state);

            for slot in state.names.slots() {
                assert_eq!(slot.load(Ordering::Acquire), 0);
            }
        });
    });
}
