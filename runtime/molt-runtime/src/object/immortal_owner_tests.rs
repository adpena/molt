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
    flags: u32,
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
        flags: header.load_metadata_flags(),
        ref_count: header.ref_count_snapshot(),
    }
}

fn assert_canonical(bits: u64) {
    let header = header_for_owned(bits);
    assert_ne!(header.flags & HEADER_FLAG_IMMORTAL, 0);
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

    fn from_intrinsic(self) -> u64 {
        let bytes: &[u8] = match self {
            Self::String => b"literal ownership: non-interned string!",
            Self::Bytes => b"literal ownership bytes",
            Self::BigInt => b"1234567890123456789012345678901234567890",
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
                Self::BigInt => bits = crate::molt_bigint_from_str(bytes.as_ptr(), len),
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
            let first = fixture.from_intrinsic();
            assert!(!crate::exception_pending(py));
            assert_eq!(header_for_owned(first).flags & HEADER_FLAG_IMMORTAL, 0);
            assert_eq!(header_for_owned(first).ref_count, 2, "creator plus cache");
            let second = fixture.from_intrinsic();
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
                assert_eq!(header_for_owned(bits).flags & HEADER_FLAG_IMMORTAL, 0);
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
                        crate::dict_set_in_place(
                            py,
                            dictionary,
                            MoltObject::from_int(index as i64).bits(),
                            bits,
                        )
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
                    let bits = fixture.from_intrinsic();
                    assert_eq!(header_for_owned(bits).flags & HEADER_FLAG_IMMORTAL, 0);
                    unsafe {
                        crate::dict_set_in_place(
                            py,
                            dictionary,
                            MoltObject::from_int(index as i64 + 10).bits(),
                            bits,
                        )
                    };
                    crate::dec_ref_bits(py, bits);
                }
                assert!(!crate::exception_pending(py));
                assert!(
                    crate::runtime_state(py)
                        .module_cache
                        .lock()
                        .unwrap()
                        .insert("immortal_owner_fixture".to_owned(), module_bits)
                        .is_none()
                );
                // Transfer creator ownership to the real runtime module cache.
                // The transaction now exercises production teardown, including
                // module dictionaries, literal caches and final canonical roots.
            });
        });
    }
}
