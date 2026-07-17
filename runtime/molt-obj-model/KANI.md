# Kani Bounded Verification for Molt

[Kani](https://model-checking.github.io/kani/) is a bounded model checker for
Rust that uses symbolic execution to prove properties over **all** possible
inputs within a bounded domain. Unlike property-based testing (proptest), which
samples random inputs, Kani exhaustively explores the state space.

## Installation

```bash
cargo install --locked kani-verifier
cargo kani setup
```

Kani requires a compatible Rust nightly toolchain which `cargo kani setup`
installs automatically.

## Running the harnesses

### molt-obj-model (NaN-boxing & refcount model)

```bash
cd runtime/molt-obj-model
cargo kani --tests
```

The intrinsic-contract harnesses intentionally use fixed-capacity bounded list
models for list axioms instead of symbolic `Vec` growth, iterator collection,
standard-library sort/dedup calls. They also use explicit bounded equality,
type tags, and identity hash models instead of stdlib `memcmp`, SipHash, or
bit-vector-heavy hash-mixing paths. That keeps the proof obligation on Molt's
contract surface and prevents CI from spending the job budget inside
standard-library collection/hash internals or solver-expensive hash mixing.

### molt-runtime (object model, string ops)

```bash
cd runtime/molt-runtime
cargo kani --tests
```

To run a specific harness:

```bash
cargo kani --tests --harness int_roundtrip
```

## What is verified

### NaN-boxing (`molt-obj-model/tests/kani_nanbox.rs`) — 16 harnesses

| Harness                          | Property                                                        |
|----------------------------------|-----------------------------------------------------------------|
| `int_roundtrip`                  | `from_int(i).as_int() == Some(i)` for all 47-bit signed ints   |
| `float_roundtrip_non_nan`        | `from_float(f).as_float()` preserves bits for all non-NaN f64  |
| `bool_roundtrip`                 | `from_bool(b).as_bool() == Some(b)` for both booleans          |
| `none_roundtrip`                 | `MoltObject::none().is_none()` and no other type flag is set    |
| `int_tag_exclusivity`            | An int is never simultaneously float, bool, none, ptr, pending  |
| `float_tag_exclusivity`          | A float is never simultaneously int, bool, none, ptr, pending   |
| `bool_tag_exclusivity`           | A bool is never simultaneously float, int, none, ptr, pending   |
| `int_bool_different_bits`        | Int and bool never produce the same bit pattern                 |
| `int_none_different_bits`        | Int and none never produce the same bit pattern                 |
| `none_pending_different_bits`    | None and pending produce different bit patterns                 |
| `float_int_different_bits`       | Non-NaN float and int never produce the same bit pattern        |
| `nan_canonicalization`           | All NaN inputs produce `CANONICAL_NAN_BITS`                     |
| `pointer_lower_48_bits_preserved`| Pointer encoding preserves the lower 48 address bits            |
| `from_bits_identity_int`         | `from_bits(obj.bits()) == obj` for ints                         |
| `from_bits_identity_float`       | `from_bits(obj.bits()) == obj` for non-NaN floats               |
| `as_int_unchecked_agrees`        | `as_int_unchecked()` matches `as_int().unwrap()` for valid ints |

### Refcount transitions (`molt-obj-model/tests/kani_refcount.rs`) — 6 harnesses

| Harness                          | Property                                                        |
|----------------------------------|-----------------------------------------------------------------|
| `successful_retain_is_exact_nonwrapping_addition` | Every accepted retain is an exact nonwrapping addition; an empty live batch is an exact no-op |
| `retain_rejects_zero_immortal_deallocating_and_overflow` | Every forbidden retain state has one exact failure classification |
| `live_upgrade_is_exactly_checked_single_retain` | Weak/live upgrade is the canonical checked one-owner transition |
| `release_reaches_zero_exactly_from_one` | Terminal death occurs if and only if release observes one owner |
| `retain_then_release_restores_every_legal_state` | A legal single retain/release pair restores its starting state |
| `revival_window_accepts_only_the_two_owned_baselines` | Finalizer revival opens only from zero or one stable ABI-view hold |

### Object model (`molt-runtime/tests/kani_object.rs`) — 26 harnesses

| Harness                              | Property                                                            |
|--------------------------------------|---------------------------------------------------------------------|
| `header_size_matches_shared_abi`     | `MoltHeader` matches `molt-codegen-abi::HEADER_SIZE_BYTES`          |
| `header_layout_preserves_alloc_alignment` | Header size/alignment preserve the generated allocation contract |
| `type_id_at_offset_zero`            | `type_id` field sits at byte offset 0                               |
| `refcount_at_offset_4`              | `ref_count` field sits at byte offset 4                             |
| `flags_at_offset_8`                 | `flags` field sits at byte offset 8                                 |
| `aux_fields_match_shared_abi_offsets` | Aux discriminator/word match generated negative offsets from payload |
| `header_from_obj_ptr_roundtrip`     | `obj_ptr - HEADER_SIZE` recovers the original header pointer        |
| `type_ids_are_unique`               | All 49 type IDs are distinct                                        |
| `header_flags_are_independent`      | All 17 flags occupy distinct single-bit positions (no overlap)      |
| `aux_kinds_and_class_word_tags_are_disjoint` | Aux and inline-class tag domains cannot alias                 |
| `immortal_flag_preserves_other_bits`| Setting IMMORTAL does not disturb other flag bits                    |
| `atomic_fetch_or_preserves_disjoint_bits` | Flag publication preserves unrelated concurrent state          |
| `atomic_fetch_and_consumes_one_flag` | Clearing one flag leaves every sibling bit intact                  |
| `atomic_compare_exchange_publishes_coherent_task_transition` | CAS exposes one coherent task-state transition       |
| `gc_publication_preserves_initialized_sibling_flags` | GC publication clears only UNPUBLISHED with release/acquire visibility |
| `alloc_obj_ptr_is_8_aligned`        | Object pointer is 8-aligned when header pointer is 8-aligned        |
| `total_size_at_least_header`        | Total allocation size >= HEADER_SIZE for any payload                |
| `range_len_step_zero`               | `range_len_i64(_, _, 0) == 0`                                      |
| `range_len_empty_ascending`         | `range_len_i64(start, stop, step) == 0` when start >= stop, step>0 |
| `range_len_empty_descending`        | `range_len_i64(start, stop, step) == 0` when start <= stop, step<0 |
| `range_len_non_negative`            | Result is always >= 0 (bounded domain)                              |
| `range_len_single_element`          | `range(start, start+1, 1)` has length 1                            |
| `range_len_single_element_descending`| `range(start, start-1, -1)` has length 1                          |
| `range_len_zero_to_n`               | `range(0, n, 1)` has length n for positive n                       |
| `range_len_matches_ceil_div`        | `range(0, n, step)` length equals `ceil(n/step)`                   |
| `finalizer_flag_idempotent`         | Setting `FINALIZER_RAN` twice is idempotent                         |

### String operations (`molt-runtime/tests/kani_string_ops.rs`) — 13 harnesses

| Harness                              | Property                                                            |
|--------------------------------------|---------------------------------------------------------------------|
| `byte_find_empty_window`            | Search over empty window returns None                                |
| `byte_find_result_is_correct`       | If find returns idx, then `haystack[idx] == needle`                 |
| `byte_find_returns_first_match`     | Returned index is the first occurrence in the window                 |
| `byte_find_oob_returns_none`        | Out-of-bounds start/end returns None (no panic)                     |
| `clamp_range_bounded`               | Clamped range satisfies `0 <= start <= stop <= len`                 |
| `clamp_range_negative_start`        | Negative start clamps to 0                                          |
| `clamp_range_stop_beyond_len`       | Stop beyond len clamps to len                                        |
| `clamp_range_inverted_is_empty`     | When start > stop, result is an empty range                          |
| `ascii_is_char_boundary`            | All ASCII bytes are UTF-8 char boundaries                            |
| `continuation_byte_not_boundary`    | Continuation bytes (0x80..0xBF) are not char boundaries              |
| `leading_byte_is_boundary`          | Leading multi-byte bytes (0xC0+) are char boundaries                 |
| `char_boundary_matches_std`         | `is_utf8_char_boundary` agrees with std on all 256 byte values       |
| `ascii_slice_preserves_utf8`        | Slicing all-ASCII at any bounds preserves UTF-8 validity             |

## Total harness count

| Crate            | File                        | Harnesses |
|------------------|-----------------------------|-----------|
| molt-obj-model   | `tests/kani_nanbox.rs`      | 45        |
| molt-obj-model   | `tests/kani_refcount.rs`    | 6         |
| molt-runtime     | `tests/kani_object.rs`      | 26        |
| molt-runtime     | `tests/kani_string_ops.rs`  | 13        |
| **Total**        |                             | **90**    |

## What is NOT yet verified

- **Pointer round-trip through the registry** (`from_ptr` / `as_ptr`): the
  pointer registry uses global mutable state (`OnceLock`, `RwLock`, `HashMap`)
  which Kani cannot easily model. The pointer harness verifies bit-level
  encoding only.
- **Exhaustive concurrent schedules**: Kani proves the shared transition
  algebra and native `free-threaded` builds stress-test concurrent
  retain/release, but a Loom-class proof of every atomic interleaving remains.
- **Actual allocation via `alloc_object`**: requires the GIL token and global
  runtime state. The harnesses model the alignment and layout invariants
  instead.
- **Target storage execution under a model checker**: GIL/default native and
  wasm use `Cell<u32>` while native `free-threaded` uses atomics; all consume
  the same proved algebra, but Kani does not execute each storage adapter.
- **Full `dec_ref_ptr` deallocation path**: involves the GIL, type-specific
  destructors, weakrefs, revival, bridge custody, and the allocator. The
  harnesses prove the transition kernel; focused runtime tests cover the
  integrated path.

## CI integration

Kani harnesses are gated behind `#[cfg(kani)]` so they are invisible to
`cargo test` and `cargo build`. They only run under `cargo kani`. To add
Kani to CI, install the verifier in the CI image and run:

```yaml
- name: Kani verification
  run: |
    cargo install --locked kani-verifier
    cargo kani setup
    cd runtime/molt-obj-model && cargo kani --tests
    cd ../molt-runtime && cargo kani --tests
```
