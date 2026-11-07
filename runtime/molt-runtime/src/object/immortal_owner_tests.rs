//! Ordinary edges never own mortality; only the canonical root bank does.

use super::*;
use crate::const_data_cache::{
    ConstDataLiteralKind, clear_const_data_literal_caches, const_data_literal_insert,
};
use crate::test_support::RuntimeTestTransaction;
use num_bigint::BigInt;
use std::sync::atomic::{AtomicU64, Ordering};

fn canonical_values(py: &PyToken<'_>) -> [u64; 4] {
    [
        MoltObject::from_ptr(crate::alloc_bytes(py, b"")).bits(),
        crate::missing_bits(py),
        crate::not_implemented_bits(py),
        crate::ellipsis_bits(py),
    ]
}

struct OwnedHeaderSnapshot {
    metadata_flags: u32,
    ref_count: u32,
}

fn header_for_owned(bits: u64) -> OwnedHeaderSnapshot {
    let ptr = crate::obj_from_bits(bits)
        .as_ptr()
        .expect("owned heap value");
    // Debug regression runs fail through provenance before inspecting an old
    // allocation if a generic edge illegally freed its canonical owner.
    #[cfg(debug_assertions)]
    assert!(
        molt_obj_model::is_registered_ptr(ptr),
        "owned pointer was retired"
    );
    let header = unsafe { &*header_from_obj_ptr(ptr) };
    OwnedHeaderSnapshot {
        metadata_flags: header.load_metadata_flags(),
        ref_count: header.ref_count_snapshot(),
    }
}

fn assert_canonical(bits: u64) {
    let header = header_for_owned(bits);
    assert_ne!(header.metadata_flags & HEADER_FLAG_IMMORTAL, 0);
    assert_eq!(header.ref_count, molt_codegen_abi::IMMORTAL_REFCOUNT);
}

#[test]
fn canonical_special_singletons_publish_real_immortal_custody() {
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for bits in canonical_values(py) {
            assert_canonical(bits);
            crate::inc_ref_bits(py, bits);
            crate::dec_ref_bits(py, bits);
            assert_canonical(bits);
        }
    });
}

#[test]
fn canonical_abi_views_preserve_runtime_immortality_on_both_crossings() {
    use molt_cpython_abi::api::refcount::{Py_DECREF, Py_INCREF};
    use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

    // Each mode owns a fresh runtime. The new interned identifier below proves
    // first publication even if bootstrap already exposed a fixed singleton.
    for owned in [false, true] {
        RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil(|py| unsafe {
                let fresh = MoltObject::from_ptr(crate::alloc_string(
                    &py,
                    b"canonical_abi_first_publication_fixture",
                ))
                .bits();
                assert!(
                    !(*header_from_obj_ptr(MoltObject::from_bits(fresh).as_ptr().unwrap()))
                        .has_flag(HEADER_FLAG_HAS_ABI_VIEW)
                );
                let values = [
                    MoltObject::from_ptr(crate::alloc_tuple(&py, &[])).bits(),
                    MoltObject::from_ptr(crate::alloc_string(&py, b"")).bits(),
                    MoltObject::from_ptr(crate::alloc_bytes(&py, b"")).bits(),
                    MoltObject::from_ptr(crate::alloc_string(&py, b"x")).bits(),
                    crate::missing_bits(&py),
                    fresh,
                ];
                for bits in values {
                    assert_canonical(bits);
                    let view = if owned {
                        GLOBAL_BRIDGE.owned_handle_to_pyobj(bits)
                    } else {
                        GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits)
                    };
                    assert!(!view.is_null());
                    let refs = (*view).ob_refcnt;
                    assert!(molt_cpython_abi::abi_types::is_immortal_refcnt(refs));
                    Py_INCREF(view);
                    Py_DECREF(view);
                    crate::dec_ref_bits(&py, bits);
                    assert_eq!((*view).ob_refcnt, refs);
                    assert_canonical(bits);
                    assert_eq!(GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits), view);
                }
                assert_eq!(
                    crate::object::gc::collect_cycles(&py).status,
                    crate::object::gc::GcCollectStatus::Completed
                );
            })
        });
        // The lifecycle transaction also exercises shutdown rebasing of the views.
    }
}

#[test]
fn ordinary_dict_and_atomic_cache_edges_preserve_canonical_root_owners() {
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let values = canonical_values(py);
        for (index, bits) in values.into_iter().enumerate() {
            assert_canonical(bits);
            let key = MoltObject::from_int(index as i64).bits();
            let dictionary = crate::alloc_dict_with_pairs(py, &[key, bits]);
            assert!(!dictionary.is_null());
            crate::inc_ref_bits(py, bits);
            let slot = AtomicU64::new(bits);
            unsafe { crate::dict_clear_in_place_shutdown(py, dictionary) };
            assert_canonical(bits);
            assert!(crate::state::cache::clear_atomic_bits(py, &slot));
            assert_eq!(slot.load(Ordering::Acquire), 0);
            assert_canonical(bits);
            crate::dec_ref_bits(py, MoltObject::from_ptr(dictionary).bits());
            assert_eq!(canonical_values(py)[index], bits);
        }
    });
}

#[derive(Clone, Copy)]
enum LiteralFixture {
    String,
    Bytes,
    BigInt,
}

impl LiteralFixture {
    fn kind(self) -> ConstDataLiteralKind {
        match self {
            Self::String => ConstDataLiteralKind::String,
            Self::Bytes => ConstDataLiteralKind::Bytes,
            Self::BigInt => ConstDataLiteralKind::BigInt,
        }
    }

    fn construct_from_intrinsic(self) -> u64 {
        let bytes: &[u8] = match self {
            Self::String => b"literal ownership: non-interned string!",
            Self::Bytes => b"literal ownership bytes",
            Self::BigInt => &[
                0x03, 0xa0, 0xc9, 0x20, 0x75, 0xc0, 0xdb, 0xf3, 0xb8, 0xac, 0xbc, 0x5f, 0x96, 0xce,
                0x3f, 0x0a, 0xd2,
            ],
        };
        // These data-segment constructors take an unboxed byte count, not a
        // tagged Python integer. All three fixtures share that ABI contract.
        let len = bytes.len() as u64;
        let mut bits = 0;
        unsafe {
            match self {
                Self::String => assert_eq!(
                    crate::molt_string_from_bytes(bytes.as_ptr(), len, &mut bits),
                    0
                ),
                Self::Bytes => assert_eq!(
                    crate::molt_bytes_from_bytes(bytes.as_ptr(), len, &mut bits),
                    0
                ),
                Self::BigInt => bits = crate::molt_bigint_from_bytes(bytes.as_ptr(), len),
            }
        }
        bits
    }

    fn fresh(self, py: &PyToken<'_>, index: usize) -> u64 {
        match self {
            Self::String => MoltObject::from_ptr(crate::alloc_string(
                py,
                format!("literal #{index}!").as_bytes(),
            ))
            .bits(),
            Self::Bytes => MoltObject::from_ptr(crate::alloc_bytes(py, &[index as u8, 42])).bits(),
            Self::BigInt => crate::builtins::numbers::bigint_bits(
                py,
                (BigInt::from(1_u8) << 100) + BigInt::from(index),
            ),
        }
    }
}

#[test]
fn literal_intrinsics_keep_cache_and_returned_owners_mortal() {
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        clear_const_data_literal_caches(py);
        for fixture in [
            LiteralFixture::String,
            LiteralFixture::Bytes,
            LiteralFixture::BigInt,
        ] {
            let first = fixture.construct_from_intrinsic();
            assert!(!crate::exception_pending(py));
            assert_eq!(
                header_for_owned(first).metadata_flags & HEADER_FLAG_IMMORTAL,
                0
            );
            assert_eq!(header_for_owned(first).ref_count, 2, "creator plus cache");
            let second = fixture.construct_from_intrinsic();
            assert_eq!(second, first);
            assert_eq!(
                header_for_owned(first).ref_count,
                3,
                "cache hit owns its result"
            );
            crate::dec_ref_bits(py, first);
            assert!(clear_const_data_literal_caches(py));
            assert_eq!(
                header_for_owned(second).ref_count,
                1,
                "second caller survives cache drain"
            );
            crate::dec_ref_bits(py, second);
            // Never inspect the value after releasing the last real owner.
        }
    });
}

#[test]
fn bounded_literal_cache_eviction_releases_only_its_own_reference() {
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        clear_const_data_literal_caches(py);
        for fixture in [
            LiteralFixture::String,
            LiteralFixture::Bytes,
            LiteralFixture::BigInt,
        ] {
            // More distinct immutable source identities than the 32-slot cache.
            // Retain every creator reference so all post-eviction reads are safe.
            let sources = [[0_u8; 8]; 64];
            let mut owners = Vec::new();
            for (index, source) in sources.iter().enumerate() {
                let bits = fixture.fresh(py, index);
                const_data_literal_insert(
                    py,
                    fixture.kind(),
                    source.as_ptr() as usize,
                    source.len(),
                    bits,
                );
                assert_eq!(
                    header_for_owned(bits).metadata_flags & HEADER_FLAG_IMMORTAL,
                    0
                );
                owners.push(bits);
            }
            assert!(
                owners
                    .iter()
                    .any(|&bits| header_for_owned(bits).ref_count == 1),
                "cache must have evicted at least one owner"
            );
            assert!(
                owners
                    .iter()
                    .any(|&bits| header_for_owned(bits).ref_count == 2),
                "cache still retains its current entries"
            );
            assert!(clear_const_data_literal_caches(py));
            for bits in owners {
                assert_eq!(header_for_owned(bits).ref_count, 1);
                crate::dec_ref_bits(py, bits);
            }
        }
    });
}

#[test]
fn module_roots_and_literal_caches_survive_repeated_runtime_retirement() {
    for _ in 0..2 {
        RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let _classes = crate::builtin_classes(py);
                let name =
                    MoltObject::from_ptr(crate::alloc_string(py, b"immortal_owner_fixture")).bits();
                let module = crate::alloc_module_obj(py, name);
                assert!(!module.is_null());
                crate::dec_ref_bits(py, name);
                let module_bits = MoltObject::from_ptr(module).bits();
                let dictionary = crate::obj_from_bits(unsafe { crate::module_dict_bits(module) })
                    .as_ptr()
                    .unwrap();
                for (index, bits) in canonical_values(py).into_iter().enumerate() {
                    assert_canonical(bits);
                    unsafe {
                        assert!(crate::dict_set_in_place(
                            py,
                            dictionary,
                            MoltObject::from_int(index as i64).bits(),
                            bits,
                        ))
                    };
                }
                for (index, fixture) in [
                    LiteralFixture::String,
                    LiteralFixture::Bytes,
                    LiteralFixture::BigInt,
                ]
                .into_iter()
                .enumerate()
                {
                    let bits = fixture.construct_from_intrinsic();
                    assert_eq!(
                        header_for_owned(bits).metadata_flags & HEADER_FLAG_IMMORTAL,
                        0
                    );
                    unsafe {
                        assert!(crate::dict_set_in_place(
                            py,
                            dictionary,
                            MoltObject::from_int(index as i64 + 10).bits(),
                            bits,
                        ))
                    };
                    crate::dec_ref_bits(py, bits);
                }
                assert!(!crate::exception_pending(py));
                assert!(
                    crate::builtins::module_table::replace_module_owned_for_test(
                        py,
                        "immortal_owner_fixture",
                        Some(module_bits)
                    )
                    .is_none()
                );
                // Transfer creator ownership to the real runtime module cache.
                // The transaction now exercises production teardown, including
                // module dictionaries, literal caches and final canonical roots.
            });
        });
    }
}

#[test]
fn custody_flags_place_the_count_word_under_runtime_custody() {
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let bits = MoltObject::from_ptr(crate::alloc_bytes(py, b"custody probe")).bits();
        let ptr = crate::obj_from_bits(bits).as_ptr().expect("heap value");
        let header = unsafe { &*header_from_obj_ptr(ptr) };
        assert!(!header.ref_count.has_runtime_custody());
        for flag in [HEADER_FLAG_HAS_ABI_VIEW, HEADER_FLAG_DEALLOCATING] {
            header.fetch_or_flags(flag);
            assert!(header.ref_count.has_runtime_custody(), "{flag:#x}");
            assert_eq!(header.ref_count_snapshot(), 1);
            header.fetch_and_flags(!flag);
            assert!(!header.ref_count.has_runtime_custody(), "{flag:#x}");
        }
        // Retiring one custody flag keeps the other's custody.
        header.update_flags(HEADER_FLAG_HAS_ABI_VIEW | HEADER_FLAG_DEALLOCATING, 0);
        header.fetch_and_flags(!HEADER_FLAG_HAS_ABI_VIEW);
        assert!(header.ref_count.has_runtime_custody());
        assert_eq!(
            header.take_flags(HEADER_FLAG_DEALLOCATING),
            HEADER_FLAG_DEALLOCATING
        );
        assert!(!header.ref_count.has_runtime_custody());
        // Unrelated flags never touch the count word.
        header.fetch_or_flags(HEADER_FLAG_FINALIZER_RAN);
        assert!(!header.ref_count.has_runtime_custody());
        header.fetch_and_flags(!HEADER_FLAG_FINALIZER_RAN);
        // A refused C-view publication leaves no custody of its own.
        header.fetch_or_flags(HEADER_FLAG_DEALLOCATING);
        assert!(!header.try_set_flags_unless(HEADER_FLAG_HAS_ABI_VIEW, HEADER_FLAG_DEALLOCATING));
        header.fetch_and_flags(!HEADER_FLAG_DEALLOCATING);
        assert!(!header.ref_count.has_runtime_custody());
        // An accepted publication goes through the flag CAS authority.
        assert!(header.try_set_flags_unless(HEADER_FLAG_HAS_ABI_VIEW, HEADER_FLAG_DEALLOCATING));
        assert!(header.ref_count.has_runtime_custody());
        assert_eq!(header.ref_count_snapshot(), 1);
        header.fetch_and_flags(!HEADER_FLAG_HAS_ABI_VIEW);
        assert!(!header.ref_count.has_runtime_custody());
        // Plain runtime transitions keep the owner count exact.
        crate::inc_ref_bits(py, bits);
        assert_eq!(header.ref_count_snapshot(), 2);
        crate::dec_ref_bits(py, bits);
        assert_eq!(header.ref_count_snapshot(), 1);
        crate::dec_ref_bits(py, bits);
    });
}

#[test]
fn custody_reconciliation_repairs_stale_failure_and_preserves_immortality() {
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let ptr = crate::alloc_bytes(py, b"custody reconciliation");
        let bits = MoltObject::from_ptr(ptr).bits();
        let header = unsafe { &*header_from_obj_ptr(ptr) };
        // A failed CAS speculates custody for a flag it never published.
        header.fetch_or_flags(HEADER_FLAG_FINALIZER_RAN);
        assert!(
            header
                .compare_exchange_synchronized_flags(0, HEADER_FLAG_HAS_ABI_VIEW)
                .is_err()
        );
        assert!(!header.ref_count.has_runtime_custody());
        header.fetch_and_flags(!HEADER_FLAG_FINALIZER_RAN);
        // A remover's stale observation precedes a completed sibling publisher.
        header.fetch_or_flags(HEADER_FLAG_HAS_ABI_VIEW);
        header.refcount_custody_after(0, HEADER_FLAG_DEALLOCATING, 0);
        assert!(header.ref_count.has_runtime_custody());
        assert_eq!(header.ref_count_snapshot(), 1);
        header.fetch_and_flags(!HEADER_FLAG_HAS_ABI_VIEW);
        header.make_immortal();
        assert_eq!(
            header.ref_count_snapshot(),
            molt_codegen_abi::IMMORTAL_REFCOUNT
        );
        header.fetch_and_flags(!HEADER_FLAG_IMMORTAL);
        assert!(
            header.ref_count.has_runtime_custody(),
            "generic flag clear cannot demote a sentinel"
        );
        assert_eq!(
            header.ref_count_snapshot(),
            molt_codegen_abi::IMMORTAL_REFCOUNT
        );
        header.fetch_or_flags(HEADER_FLAG_IMMORTAL);
        header.make_mortal_for_shutdown();
        assert_eq!(header.ref_count_snapshot(), 1);
        assert!(header.ref_count.has_runtime_custody());
        header.fetch_and_flags(!HEADER_FLAG_IMMORTAL);
        assert!(!header.ref_count.has_runtime_custody());
        crate::dec_ref_bits(py, bits);
    });
}

#[cfg(all(not(target_arch = "wasm32"), feature = "free-threaded"))]
#[test]
fn atomic_owners_remain_exact_during_custody_publication() {
    use std::sync::{Arc, Barrier};
    let header = Arc::new(MoltHeader {
        type_id: TYPE_ID_BYTES,
        ref_count: MoltRefCount::new(7),
        flags: MoltFlags::new(0),
        size_class: 0,
        aux_kind: HEADER_AUX_KIND_NONE,
        aux: MoltAuxWord::new(0),
    });
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for publisher in [false, true] {
        let header = Arc::clone(&header);
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            for _ in 0..1000 {
                if publisher {
                    header.fetch_or_flags(HEADER_FLAG_HAS_ABI_VIEW);
                    header.fetch_and_flags(!HEADER_FLAG_HAS_ABI_VIEW);
                } else {
                    header.ref_count.retain_owned(1, || false).unwrap();
                    let release = header.ref_count.release_owned().unwrap();
                    assert!(!release.reached_zero());
                }
            }
        }));
    }
    barrier.wait();
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(header.ref_count_snapshot(), 7);
    header.fetch_or_flags(HEADER_FLAG_HAS_ABI_VIEW);
    assert!(header.ref_count.has_runtime_custody());
    header.fetch_and_flags(!HEADER_FLAG_HAS_ABI_VIEW);
    assert!(!header.ref_count.has_runtime_custody());
    assert_eq!(header.ref_count_snapshot(), 7);
}

#[test]
fn reused_host_buffers_never_return_a_stale_literal() {
    // The WASM JS hosts hand one transient buffer to these constructors and
    // reuse its address: equal pointer and length must not mean equal bytes.
    let _transaction = RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        clear_const_data_literal_caches(py);
        for kind in [ConstDataLiteralKind::String, ConstDataLiteralKind::Bytes] {
            let mut buffer = *b"host buffer, first contents!";
            let mut construct = |buffer: &[u8]| {
                let mut bits = 0;
                let status = unsafe {
                    match kind {
                        ConstDataLiteralKind::String => crate::molt_string_from_bytes(
                            buffer.as_ptr(),
                            buffer.len() as u64,
                            &mut bits,
                        ),
                        _ => crate::molt_bytes_from_bytes(
                            buffer.as_ptr(),
                            buffer.len() as u64,
                            &mut bits,
                        ),
                    }
                };
                assert_eq!(status, 0);
                bits
            };
            let first = construct(&buffer);
            buffer.copy_from_slice(b"host buffer, other contents!");
            let second = construct(&buffer);
            let ptr = crate::obj_from_bits(second).as_ptr().unwrap();
            let payload = unsafe {
                match kind {
                    ConstDataLiteralKind::String => std::slice::from_raw_parts(
                        crate::object::string_bytes(ptr),
                        crate::object::string_len(ptr),
                    ),
                    _ => std::slice::from_raw_parts(
                        crate::object::bytes_data(ptr),
                        crate::object::bytes_len(ptr),
                    ),
                }
            };
            assert_eq!(payload, b"host buffer, other contents!");
            crate::dec_ref_bits(py, first);
            crate::dec_ref_bits(py, second);
        }
        clear_const_data_literal_caches(py);
    })
}
