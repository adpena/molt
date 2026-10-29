use super::{
    INTERNED_NAME_SLOT_COUNT, InternedNames, METHOD_CACHE_SLOT_COUNT, MethodCache,
    RUNTIME_STATIC_NAME_SLOT_COUNT, RuntimeStaticNames, clear_method_cache,
    clear_runtime_static_names, intern_runtime_static_name,
};
use crate::BuiltinExc;
use crate::{MoltObject, alloc_string, runtime_state};
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
            BuiltinExc::MemoryError,
            "original bridge allocation failure",
        );
        let original = crate::exception_last_bits_noinc(py);
        for key in [b"__float__".as_slice(), b"unsupported-name"] {
            assert_eq!(super::intern_bridge_protocol_name(py, key), Err(()));
        }
        assert_eq!(crate::exception_last_bits_noinc(py), original);
        let _ = crate::molt_exception_clear();
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
fn interned_name_slots_are_manifest_complete_and_unique() {
    let names = InternedNames::new();
    let slots = names.slots();
    assert_eq!(slots.len(), INTERNED_NAME_SLOT_COUNT);

    let mut seen = HashSet::with_capacity(slots.len());
    for slot in slots {
        let addr = slot as *const AtomicU64 as usize;
        assert!(seen.insert(addr));
        assert_eq!(slot.load(Ordering::Acquire), 0);
    }
}

#[test]
fn runtime_static_name_slots_are_manifest_complete_and_unique() {
    let names = RuntimeStaticNames::new();
    let slots = names.slots();
    assert_eq!(slots.len(), RUNTIME_STATIC_NAME_SLOT_COUNT);

    let mut seen = HashSet::with_capacity(slots.len());
    for slot in slots {
        let addr = slot as *const AtomicU64 as usize;
        assert!(seen.insert(addr));
        assert_eq!(slot.load(Ordering::Acquire), 0);
    }

    assert!(names.slot_for(b"__spec__").is_some());
    assert!(names.slot_for(b"path_importer_cache").is_some());
    assert!(names.slot_for(b"certfile").is_some());
    assert!(names.slot_for(b"_ORIGIN").is_some());
    assert!(names.slot_for(b"get_filename").is_some());
    assert!(names.slot_for(b"missing-runtime-static-name").is_none());
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
fn clear_runtime_static_names_releases_every_manifest_slot() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let state = runtime_state(_py);
        clear_runtime_static_names(_py, state);

        let spec_bits = intern_runtime_static_name(_py, b"__spec__");
        let cache_bits = intern_runtime_static_name(_py, b"path_importer_cache");
        assert_ne!(spec_bits, 0);
        assert_ne!(cache_bits, 0);
        assert_ne!(spec_bits, cache_bits);

        clear_runtime_static_names(_py, state);

        for slot in state.runtime_static_names.slots() {
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }
    });
}
