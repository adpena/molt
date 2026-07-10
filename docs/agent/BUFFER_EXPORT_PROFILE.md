# Py_buffer Export→Release Profile + Allocation Gate — BUFFER-EXPORT-PERF

M10 attestation for the buffer-protocol export hot path
(`runtime/molt-cpython-abi/src/api/buffer.rs`). **PROFILE FIRST** (M10): the
numbers below are measured, machine-checkable, and reproducible; the
optimization is scoped by what the profile *proves* hot, not by feel.

- Host: Windows 11, AMD Ryzen 9 3900X (12C/24T), canonical volume `C:\Molt`.
- Checkout: `perf/buffer-export-profile` worktree @ base `ce6fc4d234` (= `origin/main`).
- Toolchain: `rustc 1.96.1`, `--release` profile (`opt-level` + debuginfo).
- Method: in-crate `#[cfg(test)]` bench module
  `runtime/molt-cpython-abi/src/buffer_export_bench.rs` with a **counting global
  allocator** (System-backed, test-binary-only — ZERO production cost). The real
  C entrypoints are exercised: `copy_pybuffer_for_memoryview` (memoryview /
  numpy path → `descriptor_from_pybuffer` → `install_buffer_internal`) and
  `PyBuffer_FillInfo` (raw 1-D public path), each followed by the C-visible
  descriptor reads (`format` walk + `shape`/`strides`) and `PyBuffer_Release`.
  `obj` is NULL so no runtime hooks / bridge locks / refcount are required — the
  box + registry + release cycle is measured in isolation. N = 200 000/variant,
  warmup 4096, per-dtype + ndim sweep.

## Reproduce

```
# machine-checkable allocation-budget GATE (runs in normal cargo test):
cargo test -p molt-lang-cpython-abi --release --lib buffer_export_allocation_budget

# wall-clock profile (attestation numbers):
cargo test -p molt-lang-cpython-abi --release --lib buffer_export_timing_profile \
    -- --ignored --nocapture
```

## The hot path + Big-O

Every `numpy` array ↔ `memoryview` and every buffer-protocol export in compute
goes through `install_buffer_internal`:

1. `Box::new(BufferInternal)` — heap-boxes **1112 B** (8 B `release_kind` +
   `MoltBufferView` 1104 B) *per export*, with a wholesale ~1104 B `memcpy` into
   the box.
2. `register_buffer_internal` — inserts the box pointer into a **global
   `Mutex<HashSet<usize>>`** (`BUFFER_INTERNAL_REGISTRY`).
3. C reads `view.format` / `view.shape` / `view.strides` (raw pointers into the
   boxed descriptor) for the view's lifetime.
4. `PyBuffer_Release` — `unregister_buffer_internal` (mutex remove) +
   `Box::from_raw` drop (heap free).

**Big-O.** `MoltBufferView.shape`/`.strides` are fixed inline `[isize; 64]`
arrays, so the descriptor is **O(1) = 1112 B in space regardless of `ndim`** —
there is **no per-dim heap allocation**. The only `ndim`-dependent work is the
O(ndim) copy loop in `descriptor_from_pybuffer` (bounded `ndim ≤ 64`), measured
below to be ~2–3 ns/dim (negligible). Space per export: **exactly 1 allocation
of 1112 B**, dtype- and ndim-independent.

## Measured breakdown (baseline @ `ce6fc4d234`)

| variant | ns/export | allocs/export | bytes/export |
|---|---:|---:|---:|
| 1-D contiguous, per numpy dtype (`B b H h I i L l Q q f d`) | **142–145** | 1.000 | 1112 |
| `PyBuffer_FillInfo` raw 1-D public path | **114** | 1.000 | 1112 |

**Cost attribution controls** (isolating the ~143 ns total):

| control | ns | share of total | what it isolates |
|---|---:|---:|---|
| A: `Box::<[u8;1112]>` new+read+drop | **46** | 32% | raw malloc+free+memcpy floor |
| B: `Mutex<HashSet>` insert+remove | **41** | 29% | the registry cost / export |
| C: `MoltBufferView::default()` +read | **5** | 3% | 1104 B inline zero-init |
| residual (descriptor build + 2nd memcpy + `apply_molt_view` + contiguity scans + C reads) | ~51 | 36% | — |

**ndim sweep (f64, strided):** 1-D 144 · 2-D 148 · 3-D 152 · 4-D 152 ns/export
→ near-flat, confirming the O(ndim) term is a small constant, not a hotspot.

## Verdict: the path IS hot, but two of the three floated levers are dead

The profile **redirects** the optimization:

- **Format-string interning — REJECTED by the numbers.** ns/export is
  **dtype-independent** (142–145 ns across all 12 numpy scalar dtypes). The
  format code for numpy scalars is ≤2 chars; the `CStr` walk/copy is lost in the
  noise. A `'static` format table would save ~0 ns. (This also sidesteps the
  Miri-C constraint from `56e1c97d4b` — no format-table change is warranted at
  all.)
- **shape/strides inline small-vec — REJECTED by the numbers.** ndim is
  near-flat (144→152 ns, 1-D→4-D). shape/strides are *already* inline fixed
  arrays with no per-dim allocation; there is nothing to small-vec.
- **The two REAL costs, co-dominant at ~30% each:**
  1. **The global registry `Mutex<HashSet>` (~41 ns, 29%).** Its sole job is to
     tell `PyBuffer_Release` whether `view.internal` is a Molt-owned box (→
     `Box::from_raw`) vs a foreign exporter's pointer (→ `bf_releasebuffer`).
     This is a global-lock **serialization point**: under GIL-released parallel
     numpy compute, every concurrent export/release contends on ONE mutex — a
     scalability cliff on top of the per-op cost. **Highest-value lever:** replace
     the registry with a self-describing box header (a magic sentinel word +
     `release_kind` read directly from `view.internal`), removing both the mutex
     and the hash ops. Independent of the descriptor-projection lines the
     interlock lane edits.
  2. **The 1112 B heap box (~46 ns, 32%).** Second lever: pool/arena the
     `BufferInternal`s, or carry the descriptor in the `Py_buffer`'s own storage,
     to amortize malloc+free. Shrinking `MoltBufferView` is NOT free — `ndim` can
     legitimately reach 64 and CPython permits arbitrary strides, so the inline
     `[isize;64]` capacity cannot simply be cut.

## Machine-checkable gate (landed)

`buffer_export_allocation_budget` runs in normal `cargo test` and asserts
**exactly 1 alloc of 1112 B per export** for the 1-D uint8, 3-D f64, and
`FillInfo` paths (`EXPECTED_ALLOCS_PER_EXPORT = 1.0`,
`EXPECTED_BYTES_PER_EXPORT = 1112.0`). This is the perf-regression interlock:

- adding a second per-export allocation (a `Vec` for shape/strides, a `String`
  for format) → the gate FAILS;
- eliminating the box (arena/inline, the Phase-2 win) → the expected count is
  edited DOWN deliberately, and can never silently drift back up.

Deterministic and machine-independent (unlike an ns/export threshold), so it is
a sound CI gate. When the box-elimination lever lands, update
`EXPECTED_ALLOCS_PER_EXPORT` in the same commit and record the before→after
ns/export here.

## Phase-2 status: DEFERRED (coordination)

Source edits to `buffer.rs` are deferred until the array-buffer-export-interlock
lane (`origin/codex/ndarray-buffer-lease-land`, tip `fce3c67c74`, NOT yet in
`main`) lands, per the lane's own instruction — its lease wiring rewrites
`install_buffer_internal` / `PyBuffer_Release` / `descriptor_from_pybuffer`, the
exact functions the registry-elimination lever touches. Landing the profile +
gate now (new files only) avoids fighting that lane. Phase-2 plan, grounded in
the numbers above: (1) registry-mutex elimination via self-describing box header
(~41 ns + the multi-thread cliff); (2) box pooling/inline (~46 ns). Re-measure
each against this baseline; keep `cargo test` + the Miri finding-C repros green.
