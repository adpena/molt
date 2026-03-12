# SIMD/NEON Optimization Plan for Molt Runtime

**Status**: Planning
**Scope**: `runtime/molt-runtime/`, `runtime/molt-obj-model/`, `runtime/molt-backend/`
**Targets**: x86_64 (SSE2/SSE4.2/AVX2/AVX-512), ARM64 (NEON/SVE), WASM (simd128)

---

## Current SIMD Inventory

Molt already has hand-written SIMD intrinsics in several areas. This plan builds on
that foundation rather than starting from scratch.

| Function | Location | Arches | Width |
|----------|----------|--------|-------|
| `simd_max_byte_value` | `object/ops.rs:44013` | SSE2, AVX2, NEON, wasm32 | 128/256 |
| `simd_find_first_byte_diff` | `object/ops.rs:4295` | SSE2, AVX2, NEON, wasm32 | 128/256 |
| `simd_find_first_mismatch` (u64) | `object/ops.rs:7042` | SSE2, AVX2, NEON, wasm32 | 128/256 |
| `simd_replace_byte` | `builtins/strings.rs:184` | SSE2, AVX2, NEON, wasm32 | 128/256 |
| `sum_f64_simd_*` | `object/ops.rs:6950`, `builtins/math.rs:679` | SSE2, AVX2, NEON, wasm32 | 128/256 |
| `memchr_simd128` | `builtins/strings.rs:278` | wasm32 only | 128 |

Dependencies already providing SIMD: `memchr 2.8` (SSE2/AVX2/NEON), `memmem` (via memchr),
`blake2b_simd`/`blake2s_simd`, `simdutf 0.7` (non-wasm only).

---

## 1. String Operations

### 1.1 String Hashing (`hash_string_bytes`)

**Current implementation** (`object/ops.rs:43977`): Uses `simd_max_byte_value` to detect
encoding width (ASCII/Latin-1/BMP/full), then dispatches to a SipHash13 loop that
iterates char-by-char for non-ASCII strings. The ASCII fast path hashes raw bytes
directly.

**Proposed optimization**:
- For the non-ASCII BMP path (codepoints <= 0xFFFF), batch-decode UTF-8 to u16 using
  `simdutf` (already a dependency on non-wasm), then hash the u16 buffer in 16-byte
  chunks via SIMD loads into SipHash state.
- For the full-Unicode path, use SIMD UTF-8-to-u32 decoding (available in `simdutf`)
  to produce a u32 codepoint array, then feed SipHash in bulk.
- On WASM where `simdutf` is unavailable, the current scalar path is acceptable since
  WASM strings are typically short in Molt's target workloads.

**Expected speedup**: 2-4x for non-ASCII string hashing (Latin-1/BMP). ASCII path is
already optimal (raw byte hash).

**Architecture coverage**:
| ISA | Approach |
|-----|----------|
| x86_64 SSE4.2 | `simdutf` uses SSE4.2 for UTF-8 validation + transcoding |
| x86_64 AVX2 | `simdutf` 256-bit UTF-8 decode |
| ARM64 NEON | `simdutf` NEON UTF-8 decode |
| WASM | Scalar fallback (no `simdutf` on wasm32) |

**Cranelift CLIF status**: Not applicable -- this is runtime library code compiled by
`rustc`, not Cranelift-emitted. Cranelift does support `i8x16`/`i16x8`/`i32x4`/`f64x2`
SIMD types and operations (`iadd`, `band`, `bor`, `icmp`, `shuffle`, `extractlane`,
`insertlane`, `splat`, `load`, `store`) for x86 and AArch64 backends, but string
hashing is too complex to emit as CLIF SIMD.

**Complexity**: Medium. Requires integrating `simdutf` transcoding into the hash loop.
**Risk**: Low. Fallback to current path on decode failure.

### 1.2 String Comparison (`simd_find_first_byte_diff`)

**Current implementation** (`object/ops.rs:4295-4444`): Full SIMD coverage across all
four architectures (SSE2, AVX2, NEON, WASM simd128). Compares 16 or 32 bytes per
iteration, then falls back to scalar for tails.

**Proposed optimization**:
- The NEON implementation (`simd_find_first_byte_diff_neon`, line 4412) falls back to a
  scalar loop to find the exact differing byte within a 16-byte block. This can be
  improved using `vceqq_u8` -> `vmvnq_u8` -> byte extraction via `vgetq_lane_u8` or
  using the `__builtin_ctz` equivalent on the bitmask from a `vaddv` reduction.
- More concretely: use `vceqq_u8` to get match mask, then `vmvnq_u8` to invert,
  then `vshrn_n_u16` + `vget_lane_u64` to produce a bitmask, then `trailing_zeros()`
  to find the exact byte offset. This eliminates the 8-iteration scalar scan inside
  the NEON path.
- On x86_64, the current SSE2/AVX2 paths already use `_mm_movemask_epi8` which gives
  the exact byte position via `trailing_zeros()`. No change needed.

**Expected speedup**: 1.3-2x for NEON string comparison (eliminates inner scalar loop).
x86_64 paths are already optimal.

**Complexity**: Low. Single function change in NEON path.
**Risk**: Very low. The NEON bitmask extraction pattern is well-established.

### 1.3 String Search (`bytes_find_impl`, `bytes_rfind_impl`, `bytes_count_impl`)

**Current implementation** (`builtins/strings.rs:11-57`): Delegates to `memchr` crate
for single-byte search and `memmem` for multi-byte patterns. The `memchr` crate
already uses SSE2/AVX2/NEON internally.

**Proposed optimization**:
- The custom `memchr_fast` function (`strings.rs:269`) only has a WASM SIMD path and
  otherwise falls back to `memchr::memchr`. Remove the indirection -- `memchr` 2.8
  already auto-detects SIMD on x86/ARM. The custom WASM path is the only value-add
  (since `memchr` has limited WASM SIMD support).
- For `bytes_find_short` (2-4 byte needles, `strings.rs:60-170`): the current
  implementation uses `memchr_fast` to find the first byte, then scalar-checks the
  remaining bytes. For the 2-byte case, use `memchr2` to search for both the first
  and second needle bytes simultaneously, filtering matches.
- For repeated `str.count()` on the same haystack, the UTF-8 count cache
  (`utf8_cache.rs`) already amortizes. No SIMD opportunity beyond what `memmem` provides.

**Expected speedup**: Marginal (1.1x). `memchr` is already SIMD-optimized. The main
gain is eliminating the `memchr_fast` indirection overhead.

**Complexity**: Low.
**Risk**: Very low.

### 1.4 UTF-8 Codepoint Counting

**Current implementation** (`object/ops.rs:19897`): `utf8_codepoint_count_cached` checks
`bytes.is_ascii()` first (Rust std already uses SIMD for this), then falls back to
a block-indexed cache (`utf8_cache.rs`) or `utf8_count_prefix_blocked` which is a
scalar byte-width-table scan.

**Proposed optimization**:
- Use `simdutf::count_utf8` (available via the `simdutf` crate, already a non-wasm
  dependency) for the non-ASCII path. This uses SIMD to count leading bytes
  (bytes where `(b & 0xC0) != 0x80`) in bulk, avoiding the per-byte branch.
- On WASM, implement a simd128 version: `u8x16_splat(0xC0)` + `u8x16_and` + compare
  + popcount to count continuation bytes per 16-byte block.
- This replaces the `wtf8_codepoint_count_scan` scalar loop (`ops.rs:19943`) which
  branches on every byte.

**Expected speedup**: 3-5x for non-ASCII codepoint counting. ASCII detection is
already fast.

**Complexity**: Low (call `simdutf::count_utf8`). Medium for WASM simd128 intrinsic.
**Risk**: Low. `simdutf` is production-grade.

---

## 2. Collection Operations

### 2.1 List/Tuple Element Scanning (`simd_find_first_mismatch`)

**Current implementation** (`object/ops.rs:7042-7170`): Compares `&[u64]` slices
(NaN-boxed element arrays) using SIMD. SSE2 processes 2 elements (128 bits), AVX2
processes 4 elements (256 bits), NEON processes 2 elements per iteration.

**Proposed optimization**:
- **AVX-512 path**: Process 8 `u64` elements (512 bits) per iteration using
  `_mm512_loadu_si512` + `_mm512_cmpeq_epi64_mask`. The mask result from AVX-512
  is an 8-bit integer (no `movemask` needed), and `trailing_zeros()` gives the
  exact mismatch index. This is a natural extension of the existing AVX2 path.
- **NEON mismatch extraction**: The current NEON path (`find_first_mismatch_neon`) is
  not shown in the read output, but should use `vceqq_u64` + horizontal reduction
  to avoid scalar fallback within the block (same pattern as the byte diff fix).
- **Linear scan for `in` operator**: `molt_contains` for lists currently iterates
  element-by-element calling `obj_eq`. For homogeneous int/float lists, a SIMD scan
  comparing the raw u64 bits (NaN-boxed values are bitwise-identical for equal
  ints/floats) would be 4-8x faster. The check is: if the target is an inline value
  (int/bool/none/float), scan the `Vec<u64>` with SIMD for exact bit match.

**Expected speedup**: AVX-512: 1.5-2x over AVX2 for large lists. `in` operator with
SIMD scan: 4-8x for int/float membership tests.

**Architecture coverage**:
| ISA | Elements/iter | Notes |
|-----|--------------|-------|
| SSE2 | 2 | Current |
| AVX2 | 4 | Current |
| AVX-512 | 8 | New |
| NEON | 2 | Current (needs bitmask fix) |
| WASM | 2 | Current |

**Cranelift CLIF status**: Cranelift has `icmp.i64x2` and basic SIMD comparison ops.
For JIT-compiled list iteration, Cranelift could emit SIMD loads from the element
`Vec<u64>` backing store, but this requires the compiler to prove the list is not
mutated during iteration. Deferred to a future optimization phase.

**Complexity**: Medium (AVX-512 path). High (SIMD `in` operator).
**Risk**: Medium. AVX-512 frequency throttling on some Intel CPUs can cause net
slowdowns if the surrounding code is scalar. Must benchmark with realistic workloads.

### 2.2 Dict/Set Key Lookup (Hash Table Probing)

**Current implementation** (`object/ops.rs:44873-44950`): Linear probing hash table
using `Vec<usize>` as the bucket array. `dict_find_entry_fast` hashes the key, masks
to get the starting slot, then probes linearly comparing entries one at a time.
Tombstones (`TABLE_TOMBSTONE = usize::MAX`) are skipped. Load factor threshold is
70% (`new_entries * 10 >= table.len() * 7`).

**Proposed optimization -- Swiss Table-style SIMD probing**:

Molt's dict implementation is a simple open-addressing linear probe table. The primary
bottleneck is the serial probe loop, which on a cache miss requires one memory access
per probe step.

A Swiss Table (hashbrown) approach uses a separate metadata byte array where each byte
stores the top 7 bits of the hash (H2) plus an empty/deleted sentinel. SIMD is used to
compare 16 metadata bytes simultaneously:

```
// Pseudocode for SIMD probe
let h2 = (hash >> 57) as u8 | 0x80;  // Top 7 bits + high bit set
let group = load_16_metadata_bytes(ctrl + group_offset);
let matches = simd_cmpeq_u8(group, splat(h2));
for bit in matches.iter_ones() {
    let idx = group_offset + bit;
    if keys[idx] == target_key { return Found(idx); }
}
let empties = simd_cmpeq_u8(group, splat(EMPTY));
if empties.any() { return NotFound; }
// advance to next group
```

This eliminates serial probing and checks 16 slots per SIMD operation.

**However**: Molt's dict is not a simple `HashMap<K,V>`. It maintains insertion order
via the `order: Vec<u64>` (interleaved key/value pairs) with the `table: Vec<usize>`
serving as a secondary index. Migrating to Swiss Table metadata would require
restructuring both `dict_order` and `dict_table` storage.

**Recommended incremental approach**:
1. Add a `ctrl: Vec<u8>` metadata array alongside `table: Vec<usize>`.
2. Store H2 in `ctrl[slot]` during insertion.
3. Modify `dict_find_entry_fast`/`dict_find_entry_with_hash` to do SIMD group
   comparison on `ctrl` before checking `table[slot]` and `order[entry_idx * 2]`.
4. Keep the existing `order` vector for insertion-order iteration.

**Expected speedup**: 2-4x for dict lookups with > 8 entries. Small dicts (< 8 entries)
may see no improvement due to setup overhead.

**Architecture coverage**:
| ISA | Instruction | Group size |
|-----|-------------|-----------|
| SSE2 | `_mm_cmpeq_epi8` + `_mm_movemask_epi8` | 16 |
| AVX2 | `_mm256_cmpeq_epi8` + `_mm256_movemask_epi8` | 32 |
| NEON | `vceqq_u8` + bitmask extraction | 16 |
| WASM | `u8x16_eq` + `u8x16_bitmask` | 16 |

**Cranelift CLIF status**: Dict lookups are runtime calls, not Cranelift-emitted code.
CLIF SIMD support is irrelevant here.

**Complexity**: High. Requires changes to dict allocation, insertion, deletion, rebuild.
**Risk**: Medium. Must ensure insertion-order semantics are preserved exactly (Python
dict ordering guarantee). The `order` vector approach is compatible.

### 2.3 Set Membership Testing

**Current implementation**: Identical structure to dict (`set_order: Vec<u64>`,
`set_table: Vec<usize>`). Same linear probing as dict.

**Proposed optimization**: Same Swiss Table metadata approach as dicts. Sets are
simpler (no value array), so the `order` vector is single-element per entry rather
than interleaved key/value.

**Expected speedup**: Same as dict (2-4x for > 8 elements).
**Complexity**: Medium (simpler than dict, no value handling).
**Risk**: Low.

---

## 3. Numeric Operations

### 3.1 Batch Float Summation (`sum_f64_simd_*`)

**Current implementation** (`object/ops.rs:6950-7005`, `builtins/math.rs:679-758`):
Full SIMD coverage for `sum()` on float lists. SSE2 (2 lanes), AVX2 (4 lanes), NEON
(2 lanes), WASM simd128 (2 lanes).

**Proposed optimization**:
- **AVX-512**: 8 f64 lanes per iteration (`_mm512_add_pd`). Straightforward extension.
- **Fused multiply-add for `sum(x*y for ...)`**: When the compiler can prove a
  generator is a simple product reduction, emit `_mm256_fmadd_pd` (FMA3) instead of
  separate multiply + add. This is a compiler optimization (TIR -> LIR lowering),
  not a runtime change.
- **Kahan summation variant**: The current SIMD sum has floating-point ordering
  differences vs CPython. For determinism, consider a SIMD-friendly compensated
  summation (add error-compensation lane alongside accumulator lane).

**Expected speedup**: AVX-512: 1.5-2x over AVX2 for large float lists.
**Complexity**: Low (AVX-512 extension). High (FMA compiler integration).
**Risk**: Low for runtime. Medium for compiler integration (determinism concerns).

### 3.2 Vectorized Integer Arithmetic on `intarray`

**Current implementation** (`object/mod.rs:702-708`): `intarray` stores `i64` values
contiguously (`*const i64` from `ptr.add(sizeof::<usize>())`). Currently no SIMD
operations on int arrays.

**Proposed optimization**:
- Element-wise add/sub/mul/comparison on `intarray` using `_mm256_add_epi64` (4
  elements/iter) or NEON `vaddq_s64` (2 elements/iter).
- Vectorized `min`/`max` scan: `_mm256_max_epi64` (AVX-512 only, no AVX2 equivalent
  for i64 max -- use `_mm256_cmpgt_epi64` + blend on AVX2).
- Batch comparison chains: `a < b < c` lowered to simultaneous SIMD comparison of
  adjacent elements.

**Expected speedup**: 2-4x for bulk int array operations.

**Cranelift CLIF status**: Cranelift supports `iadd.i64x2`, `isub.i64x2`, `imul.i64x2`
on x86 and AArch64. The compiler could emit SIMD CLIF ops for typed int array loops
detected during TIR specialization. Requires loop vectorization analysis in the
compiler (not yet implemented).

**Complexity**: Medium (runtime intrinsics). Very high (compiler auto-vectorization).
**Risk**: Medium. Must handle overflow semantics correctly (Python ints are
arbitrary-precision; overflow to BigInt must be detected per-element).

### 3.3 NaN-Boxed Comparison Chains

**Current implementation**: Comparison chains like `a < b < c` are lowered to sequential
comparisons with short-circuit evaluation. Each comparison involves `obj_eq`/`obj_lt`
calls that check types and extract values.

**Proposed optimization**: When the compiler can prove all operands are the same inline
type (int or float), batch-compare the raw `u64` NaN-boxed bits. For ints, the NaN-box
encoding preserves ordering within the 47-bit signed range (after sign-extension), so
raw bit comparison with appropriate masking works. For floats, raw IEEE 754 bit
comparison works for positive values; negative values need sign-flipping.

**Expected speedup**: 2-3x for type-homogeneous comparison chains.
**Complexity**: High (requires type specialization in compiler).
**Risk**: Medium. Must handle mixed-type comparison edge cases.

---

## 4. Memory Operations

### 4.1 Object Allocation Zeroing

**Current implementation** (`object/mod.rs:502-527`): `alloc_object_zeroed` uses
`std::alloc::alloc_zeroed`, which delegates to the system allocator's `calloc` or
`mmap` (which returns zeroed pages). The header is then written field-by-field.

**Proposed optimization**:
- `alloc_zeroed` already uses the most efficient OS mechanism (zeroed pages from mmap,
  or `calloc` which skips zeroing for fresh pages). No SIMD improvement possible here.
- For the non-zeroed path (`alloc_object`, line 529), the header write is 6 fields
  (40 bytes). This fits in a single cache line. A SIMD store of a pre-built header
  template (using `_mm256_storeu_si256` for 32 of the 40 bytes + scalar for the
  remaining 8) would save 4 scalar stores but adds complexity for minimal gain.

**Expected speedup**: Negligible. OS-level page zeroing is already optimal.
**Complexity**: Low.
**Risk**: Very low (not worth implementing).

### 4.2 Bulk Refcount Operations

**Current implementation** (`object/mod.rs:872-904, 973-1110`): `inc_ref_ptr` and
`dec_ref_ptr` operate on individual objects. When freeing a list/tuple, a scalar loop
iterates all elements calling `dec_ref_bits` per element (lines 1070-1077, 1091-1098).

**Proposed optimization -- vectorized refcount-free for homogeneous collections**:
- When freeing a list, scan the element array with SIMD to classify elements:
  1. Load 4 u64 NaN-boxed values (AVX2).
  2. AND with `QNAN | TAG_MASK` to extract tags.
  3. Compare with `QNAN | TAG_PTR` to identify heap pointers.
  4. For inline values (int/bool/none/float), skip refcount entirely.
  5. For heap pointers, extract and batch-decrement.
- This avoids the overhead of `obj_from_bits` + `as_ptr()` + null check per element
  for inline values, which are the majority in numeric/boolean lists.

```
// Pseudocode: AVX2 batch tag check
let tag_mask = _mm256_set1_epi64x((QNAN | TAG_MASK) as i64);
let ptr_tag  = _mm256_set1_epi64x((QNAN | TAG_PTR) as i64);
let elements = _mm256_loadu_si256(elems.as_ptr() as *const __m256i);
let tags     = _mm256_and_si256(elements, tag_mask);
let is_ptr   = _mm256_cmpeq_epi64(tags, ptr_tag);
let mask     = _mm256_movemask_epi8(is_ptr);  // 4 groups of 8 bytes
// Only call dec_ref_ptr for elements where mask indicates TAG_PTR
```

**Expected speedup**: 2-4x for freeing large int/float lists. No improvement for
lists of heap objects (strings, nested lists).

**Architecture coverage**:
| ISA | Elements/iter | Tag compare |
|-----|--------------|-------------|
| SSE2 | 2 | `_mm_cmpeq_epi64` (SSE4.1) |
| AVX2 | 4 | `_mm256_cmpeq_epi64` |
| NEON | 2 | `vceqq_u64` |
| WASM | 2 | `i64x2_eq` |

**Complexity**: Medium. Need to handle the ptr-extraction path carefully.
**Risk**: Low. Inline values are definitionally safe to skip.

### 4.3 Object Pool Prefetch

**Current implementation** (`object/mod.rs:351-454`): Thread-local object pool
(`OBJECT_POOL_TLS`) stores freed objects in size-bucketed `Vec<PtrSlot>`. Pool take
and return are scalar.

**Proposed optimization**: When returning a batch of objects to the pool (e.g., during
list destruction), issue prefetch hints (`_mm_prefetch` / `__builtin_prefetch`) for
the next few pool entries to warm L1 cache for the subsequent pool takes. This is
speculative and benefits tight alloc/free cycles (iterator patterns).

**Expected speedup**: 1.1-1.2x in tight loops with high object churn.
**Complexity**: Low.
**Risk**: Very low. Prefetch is advisory; incorrect hints just waste a cache line load.

---

## 5. NaN-Boxing Optimizations

### 5.1 Batch Type Extraction

**Current implementation** (`molt-obj-model/src/lib.rs:207-245`): Type checking is done
per-value via bitwise AND + compare: `(self.0 & (QNAN | TAG_MASK)) == (QNAN | TAG_X)`.
This is already a single comparison instruction per value.

**Proposed optimization -- SIMD batch type classification**:
- When processing a slice of NaN-boxed values (e.g., during list comprehension
  building, tuple unpacking, function call argument binding), classify all values'
  types simultaneously:

```
// Classify 4 NaN-boxed u64 values at once (AVX2)
let vals      = _mm256_loadu_si256(values.as_ptr() as *const __m256i);
let tag_mask  = _mm256_set1_epi64x((QNAN | TAG_MASK) as i64);
let tags      = _mm256_and_si256(vals, tag_mask);

let is_int    = _mm256_cmpeq_epi64(tags, _mm256_set1_epi64x((QNAN | TAG_INT) as i64));
let is_float  = /* (val & QNAN) != QNAN -- requires different logic */;
let is_ptr    = _mm256_cmpeq_epi64(tags, _mm256_set1_epi64x((QNAN | TAG_PTR) as i64));
```

- Float detection is special: `(bits & QNAN) != QNAN`. This can be done with
  `_mm256_and_si256` + `_mm256_cmpeq_epi64` + invert.

**Use cases**:
- `callargs` argument type validation (batch-check all positional args are the
  expected types).
- Collection building (skip inc_ref for inline values).
- `isinstance()` batch checks in comprehensions.

**Expected speedup**: 2-4x for batch type checks on 4+ values.
**Complexity**: Medium.
**Risk**: Low. Pure classification, no mutation.

### 5.2 Batch Integer Extraction

**Current implementation** (`molt-obj-model/src/lib.rs:267-288`): `as_int()` does
tag check + sign extension per value. `as_int_unchecked()` skips the tag check.

**Proposed optimization**: Extract N integers from NaN-boxed values simultaneously:
```
// Extract 4 i64 values from NaN-boxed ints (AVX2)
let vals     = _mm256_loadu_si256(...);
let int_mask = _mm256_set1_epi64x(INT_MASK as i64);
let raw      = _mm256_and_si256(vals, int_mask);  // Strip tags
// Sign extension: shift left 17, arithmetic shift right 17
let shifted  = _mm256_slli_epi64(raw, 17);
let extended = _mm256_srai_epi64(shifted, 17);  // AVX-512 only for 64-bit arith shift
```

Note: `_mm256_srai_epi64` requires AVX-512VL. On AVX2, use the 32-bit shift trick:
split into high/low 32-bit halves, arithmetic-shift the high half, recombine.

**Expected speedup**: 3-4x for bulk int extraction (e.g., list-to-array conversion).
**Complexity**: Medium. Sign extension on AVX2 without AVX-512 is tricky.
**Risk**: Low.

---

## 6. Hash Table Probing (Swiss Table Detail)

### 6.1 Current Probing

**Current implementation** (`object/ops.rs:44873-44950`): Simple open-addressing linear
probe. The table is `Vec<usize>` where each slot stores `entry_idx + 1` (0 = empty,
`usize::MAX` = tombstone). Probe sequence:
```
slot = hash & mask;
loop {
    if table[slot] == 0 { return NotFound; }
    if table[slot] == TOMBSTONE { slot = (slot+1) & mask; continue; }
    if keys_equal(order[table[slot]-1], target) { return Found; }
    slot = (slot+1) & mask;
}
```

This is one memory access per probe step (to `table[slot]`) plus one conditional
memory access to `order[entry_idx * 2]` for key comparison. On a hash collision,
each probe step is a dependent memory load.

### 6.2 Swiss Table Migration Plan

**Phase 1 -- Metadata overlay (compatible with current layout)**:
- Add `ctrl: Vec<u8>` to dict allocation. Each byte stores:
  - `0x00..=0x7F`: empty (0x00) or deleted (0x7E).
  - `0x80..=0xFF`: occupied, low 7 bits = H2 (top 7 bits of hash).
- On insertion, set `ctrl[slot] = (hash >> 57) as u8 | 0x80`.
- On lookup, SIMD-compare 16 ctrl bytes against `splat(h2)`. Only probe `table[slot]`
  for matching positions.
- Keep `table: Vec<usize>` and `order: Vec<u64>` unchanged.

**Phase 2 -- Eliminate `table: Vec<usize>` for compact dicts**:
- For dicts with < 128 entries, the `ctrl` byte can directly encode the entry index
  (since entry indices fit in 7 bits). This eliminates the `table` indirection.
- For larger dicts, keep `table` but use `ctrl` as a Bloom filter to skip most
  probes.

**Phase 3 -- Inline small dicts**:
- Dicts with <= 4 entries: store key/value pairs inline in the object allocation
  (no heap `Vec`). SIMD-compare all 4 keys simultaneously against the lookup key.
  This eliminates hash computation for tiny dicts (common for `**kwargs`, instance
  `__dict__` with few attributes).

**Memory impact**: +1 byte per hash table slot (16 bytes per 16-slot group). For a
dict with 8 entries (16-slot table at 50% load), this adds 16 bytes. Acceptable.

**Complexity**: Phase 1: High. Phase 2: Very high. Phase 3: Medium.
**Risk**: Phase 1: Medium (must maintain insertion order). Phase 2-3: High.

---

## 7. GC/Refcount Operations

### 7.1 Vectorized Refcount Scanning

**Current implementation**: The GC design (`0009_GC_DESIGN.md`) uses reference counting
as the primary mechanism. Cycle collection is not yet implemented. `dec_ref_ptr`
(line 973) handles deallocation when refcount hits zero, recursively freeing child
objects.

**Proposed optimization -- batch refcount for list/tuple destruction**:
When a list or tuple is freed (refcount -> 0), its element array `Vec<u64>` is
iterated to decrement each element's refcount. As described in section 4.2, SIMD
can classify elements to skip inline values. Additionally:

- **Batch immortality check**: Before decrementing, check `HEADER_FLAG_IMMORTAL` on
  each element's header. Load 4 headers' `flags` fields, AND with
  `HEADER_FLAG_IMMORTAL`, compare to zero. Skip immortal objects in bulk.
- **Deferred free list**: Instead of recursively calling `dec_ref_ptr` for each element,
  collect pointers-to-free into a batch buffer. Process the buffer in FIFO order.
  This converts recursive deallocation into iterative, improving cache locality
  (headers are accessed sequentially rather than in call-stack order).

**Expected speedup**: 1.5-2x for large collection teardown.
**Complexity**: High (deferred free list changes deallocation order, which may affect
finalizer ordering).
**Risk**: Medium. Python finalizer ordering is not strictly specified, but changing it
could break programs that depend on `__del__` order. Must verify against CPython
behavior.

### 7.2 Root Set Marking (Future Cycle Collector)

**Current status**: No cycle collector exists yet. When implemented, SIMD-parallel
root set scanning would be valuable.

**Proposed approach for future cycle collector**:
- Maintain a `Vec<*mut u8>` of tracked objects (potential cycle participants).
- During mark phase, SIMD-scan the tracked set to check mark bits in headers:
  ```
  // Load 4 header pointers, gather mark bits
  for chunk in tracked.chunks(4) {
      // AVX2 gather: _mm256_i64gather_epi64 to load flags from headers
      // Compare mark bit, produce mask of unmarked objects
  }
  ```
- AVX2 gather (`_mm256_i64gather_epi64`) can load 4 non-contiguous 64-bit values
  per instruction, enabling parallel header inspection.

**Cranelift CLIF status**: Cranelift does not currently support gather/scatter
instructions. This would be runtime-only code.

**Complexity**: Very high (cycle collector is not yet designed).
**Risk**: Deferred -- blocked on cycle collector design.

---

## 8. Cache Hierarchy Optimization

### 8.1 MoltHeader Cache-Line Alignment

**Current layout** (`object/mod.rs:197-205`):
```rust
#[repr(C)]
pub struct MoltHeader {
    pub type_id: u32,       // offset 0, 4 bytes
    pub ref_count: AtomicU32, // offset 4, 4 bytes
    pub poll_fn: u64,       // offset 8, 8 bytes
    pub state: i64,         // offset 16, 8 bytes
    pub size: usize,        // offset 24, 8 bytes
    pub flags: u64,         // offset 32, 8 bytes
}   // Total: 40 bytes
```

`HEADER_SIZE_BYTES = 40` (confirmed in `molt-backend/src/lib.rs:36`). The allocation
alignment is 8 bytes (`Layout::from_size_align(total_size, 8)`).

**Analysis**: A 64-byte cache line holds one header (40 bytes) plus 24 bytes of payload.
The hot fields for type dispatch are `type_id` (offset 0) and `ref_count` (offset 4),
which are always in the same cache line as the header start. `flags` (offset 32) is
also in the same cache line for 64-byte lines but would be in a different line for
32-byte lines (some ARM cores).

**Proposed optimization**:
- Reorder fields to put the hottest fields (`type_id`, `ref_count`, `flags`) at the
  start. Move `poll_fn` and `size` (cold fields for most objects) to the end:
  ```rust
  #[repr(C)]
  pub struct MoltHeader {
      pub type_id: u32,         // offset 0  -- hot: every type dispatch
      pub ref_count: AtomicU32, // offset 4  -- hot: every inc/dec_ref
      pub flags: u64,           // offset 8  -- warm: immortality check, gen flags
      pub state: i64,           // offset 16 -- warm: class bits, hash cache
      pub size: usize,          // offset 24 -- cold: only on dealloc
      pub poll_fn: u64,         // offset 32 -- cold: only for async tasks
  }
  ```
  This ensures the three hottest fields are in the first 16 bytes (fits in a single
  16-byte fetch on ARM cores with 32-byte cache lines).

**Impact**: Requires updating `HEADER_STATE_OFFSET` in `molt-backend/src/lib.rs:37`
and all code that accesses header fields by offset (search for
`ptr.sub(std::mem::size_of::<MoltHeader>())` and `header_from_obj_ptr`).

**Expected speedup**: Marginal (< 5%). Header access is already typically a L1 hit
since objects are accessed immediately after allocation.

**Complexity**: Medium (backend codegen offset constants must be updated).
**Risk**: Medium. Any offset calculation error causes silent memory corruption.

### 8.2 NaN-Boxed Value Alignment

**Current layout**: `MoltObject` is `#[repr(transparent)]` wrapping `u64`. Element
arrays (`Vec<u64>` for lists/tuples) are naturally 8-byte aligned by Rust's allocator.

**Analysis**: `Vec<u64>` data is 8-byte aligned, and sequential access patterns (list
iteration) produce excellent spatial locality. No alignment change needed.

**Proposed optimization**: None. Current layout is cache-optimal for the access pattern.

### 8.3 Dict Table Cache-Friendliness

**Current layout**: Dict uses two heap-allocated vectors:
- `order: Vec<u64>` -- interleaved `[key0, val0, key1, val1, ...]`
- `table: Vec<usize>` -- hash table slots

These are separate heap allocations. A dict lookup touches:
1. The dict object (8 bytes: pointer to `order` Vec)
2. The `order` Vec header (24 bytes: ptr, len, cap)
3. The `table` Vec header (24 bytes: ptr, len, cap)
4. The `table` data (probe slots)
5. The `order` data (key comparison)

That is 4-5 pointer dereferences before reaching the key data.

**Proposed optimization -- inline small dicts**:
- For dicts with <= 8 entries, allocate `order` and `table` inline in the object
  payload (fixed-size arrays rather than heap-allocated Vecs). This reduces pointer
  chasing from 4 levels to 2 (object -> payload -> key).
- Threshold: inline layout needs `8 * 2 * 8 = 128` bytes for order (8 key-value
  pairs) + `16 * 8 = 128` bytes for table (16 slots at 50% load) = 256 bytes total.
  This fits in 4 cache lines.

**Expected speedup**: 1.5-2x for small dict lookups (< 8 entries). Most Python dicts
are small (instance `__dict__`, `**kwargs`).

**Complexity**: High. Requires bifurcated dict code paths (inline vs heap).
**Risk**: Medium. Must handle growth transition from inline to heap correctly.

### 8.4 Prefetch Hints for Sequential Collection Traversal

**Current implementation**: List/tuple iteration (`iter_target_bits` + `iter_index` +
`seq_vec_ref`) loads elements sequentially from the backing `Vec<u64>`. The hardware
prefetcher handles this well for contiguous access.

**Proposed optimization**:
- For dict iteration (which accesses `order[i*2]` and `order[i*2+1]` sequentially),
  prefetch the next 2 entries (32 bytes ahead) using `_mm_prefetch(..., _MM_HINT_T0)`.
- For nested data structures (list of dicts, dict of lists), prefetch the next
  element's heap pointer after extracting the current element's pointer. This hides
  the latency of following the NaN-boxed pointer to the child object's header.

```rust
// In list iteration hot loop
let bits = elements[i];
if let Some(ptr) = obj_from_bits(bits).as_ptr() {
    // Prefetch next element's target while processing current
    if i + 1 < elements.len() {
        let next_bits = elements[i + 1];
        if let Some(next_ptr) = obj_from_bits(next_bits).as_ptr() {
            unsafe { _mm_prefetch(next_ptr.sub(HEADER_SIZE) as *const i8, _MM_HINT_T0); }
        }
    }
    // ... process current element
}
```

**Expected speedup**: 1.1-1.3x for iteration over lists of heap objects. No improvement
for int/float lists (no pointer chasing).

**Complexity**: Low.
**Risk**: Very low. Prefetch is advisory.

### 8.5 False Sharing Avoidance

**Current implementation**: The GIL (`concurrency/gil.rs`) serializes all mutation.
Thread-local object pools (`OBJECT_POOL_TLS`) are per-thread `RefCell<Vec<Vec<PtrSlot>>>`.
Global shared state includes:
- `ALLOC_COUNT`, `ALLOC_STRING_COUNT`, etc. (`AtomicU64` counters)
- `PTR_REGISTRY` shards (`RwLock<HashMap<u64, PtrSlot>>`, 64 shards)
- `MethodCache` (per-method `AtomicU64` slots)

**Analysis**: The `AtomicU64` counters (`ALLOC_COUNT`, etc.) are global statics that
may share cache lines. Since the GIL serializes access, false sharing between counters
is not a correctness issue, but it can cause unnecessary cache-line bouncing when
multiple counters are updated in sequence (each counter update invalidates the shared
line on other cores that have read any counter on that line).

**Proposed optimization**:
- Pad each global counter to 64 bytes using `#[repr(align(64))]` wrapper:
  ```rust
  #[repr(align(64))]
  struct CacheAligned<T>(T);
  static ALLOC_COUNT: CacheAligned<AtomicU64> = CacheAligned(AtomicU64::new(0));
  ```
- For `MethodCache`, pad between the `AtomicU64` fields or use a single `AtomicU64`
  with bitfield encoding (reducing total cache footprint).
- For the `PtrRegistry` shards, the 64-shard design already distributes access. Each
  shard's `RwLock<HashMap>` is a separate heap allocation, so false sharing between
  shards is unlikely.

**Expected speedup**: Negligible under GIL. Meaningful (1.1-1.3x) if/when GIL-free
execution is implemented for I/O operations.

**Complexity**: Low.
**Risk**: Very low. Pure padding, no semantic change.

---

## Implementation Priority

Ordered by expected impact / effort ratio:

| Priority | Area | Expected Speedup | Effort | Section |
|----------|------|-----------------|--------|---------|
| P0 | NEON string comparison bitmask fix | 1.3-2x (ARM) | Low | 1.2 |
| P0 | UTF-8 codepoint counting via simdutf | 3-5x (non-ASCII) | Low | 1.4 |
| P1 | Batch refcount classification (tag check) | 2-4x (list free) | Medium | 4.2 |
| P1 | SIMD `in` operator for int/float lists | 4-8x | Medium | 2.1 |
| P1 | Non-ASCII string hash (simdutf transcode) | 2-4x (non-ASCII) | Medium | 1.1 |
| P2 | Swiss Table metadata for dict/set | 2-4x (dict lookup) | High | 6.2 |
| P2 | AVX-512 extensions (mismatch, sum) | 1.5-2x | Low | 2.1, 3.1 |
| P2 | Batch NaN-box type classification | 2-4x (batch ops) | Medium | 5.1 |
| P3 | Inline small dicts | 1.5-2x (small dict) | High | 8.3 |
| P3 | Prefetch hints for nested iteration | 1.1-1.3x | Low | 8.4 |
| P3 | Header field reordering | < 5% | Medium | 8.1 |
| P3 | Cache-line padding for counters | Negligible | Low | 8.5 |
| Deferred | Compiler auto-vectorization (CLIF) | Variable | Very high | 3.2, 3.3 |
| Deferred | Cycle collector SIMD scanning | N/A | Very high | 7.2 |

---

## Cranelift CLIF SIMD Support Summary

Cranelift's SIMD support (relevant for future compiler-emitted vectorized code):

| CLIF Type | x86_64 | AArch64 | Notes |
|-----------|--------|---------|-------|
| `i8x16` | SSE2+ | NEON | Full support |
| `i16x8` | SSE2+ | NEON | Full support |
| `i32x4` | SSE2+ | NEON | Full support |
| `i64x2` | SSE2+ | NEON | Full support |
| `f32x4` | SSE2+ | NEON | Full support |
| `f64x2` | SSE2+ | NEON | Full support |
| `iadd` | Yes | Yes | Integer vector add |
| `isub` | Yes | Yes | Integer vector sub |
| `imul` | i16x8, i32x4 | i16x8, i32x4 | No i64x2 multiply |
| `icmp` | Yes | Yes | All comparisons |
| `band`/`bor`/`bxor` | Yes | Yes | Bitwise ops |
| `splat` | Yes | Yes | Broadcast scalar |
| `shuffle` | Yes | Yes | Lane permutation |
| `extractlane` | Yes | Yes | Extract scalar |
| `insertlane` | Yes | Yes | Insert scalar |
| `load`/`store` | Yes | Yes | 128-bit aligned/unaligned |
| `swiden_*` | Partial | Partial | Widening ops |
| `fma` | No | Yes (NEON) | Fused multiply-add |
| Gather/Scatter | No | No | Not supported |
| 256-bit (AVX2) | No | N/A | Not supported |
| 512-bit (AVX-512) | No | N/A | Not supported |

**Key limitation**: Cranelift only supports 128-bit SIMD vectors. All AVX2/AVX-512
optimizations must be in runtime library code (compiled by `rustc`), not in
Cranelift-emitted code. This means compiler-driven vectorization is limited to 2x
speedup for f64/i64 operations (2 lanes) until Cranelift adds wider vector support.

---

## Testing Strategy

1. **Correctness**: All SIMD paths must produce bit-identical results to scalar
   fallbacks. Add property tests (`proptest`) that compare SIMD and scalar outputs
   for random inputs.
2. **Architecture coverage**: CI must test on both x86_64 and aarch64. Use
   `cfg(target_arch)` gates and ensure scalar fallbacks exist for every SIMD path.
3. **Performance validation**: Benchmark each optimization using `criterion` microbenchmarks
   before merging. Require >= 1.2x improvement to justify the code complexity.
4. **WASM parity**: Every new SIMD path must have a wasm32-simd128 implementation or
   an explicit scalar fallback. Use `cfg!(target_feature = "simd128")` runtime checks.
5. **Determinism**: All SIMD numeric operations must produce deterministic results
   across architectures. Float SIMD summation order must be documented (SIMD sum is
   not associative; the lane reduction order must be fixed).
