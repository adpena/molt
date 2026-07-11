# Py_buffer Export→Release Profile + Allocation Gate — BUFFER-EXPORT-PERF

> **LANDED — BUFFER-DISTILL-55 (2026-07-10).** Both Phase-2 levers below are
> DONE, by deletion rather than by a faster registry (see the addendum at the
> bottom for the measured before→after):
>
> - `BUFFER_INTERNAL_REGISTRY` (global `Mutex<HashSet>`) is **deleted**.
>   `PyBuffer_Release` discriminates on the **exporter object** — CPython's
>   model (`Objects/abstract.c` dispatches on `view->obj`) — via the bridge's
>   `molt_handle_for_pyobj` (genuine molt handles only; raw-registered C
>   objects classify as foreign, so export/release dispatch can never
>   disagree). Foreign `view.internal` cookies are never dereferenced.
> - The 1112 B `BufferInternal` box is **deleted**. `PyBuffer_FillInfo` is
>   CPython-exact and **allocation-free** (static `"B"` format,
>   self-referential `shape = &view.len` / `strides = &view.itemsize`,
>   `internal = NULL`); memoryviews embed the descriptor in the
>   `PyMemoryViewObject` itself (CPython's `ob_array` model) and are filled
>   **in place** (never filled on the stack and moved — the field-trick UAF
>   class from the reverted `7da58cff8f`); molt-native `PyObject_GetBuffer`
>   installs a right-sized `ExportInternal` (32 B + 16 B/dim, raw-projection
>   provenance, Miri SB+TB clean) whose header carries `owner` for the runtime
>   pin release.
>
> The measured-breakdown/verdict sections below are the **pre-distill**
> baseline record, kept for attribution history.

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

## Phase-2 status: UNBLOCKED (interlock landed; buffer.rs untouched)

The array-buffer-export-interlock lane **landed on `main` at `8fe41579af`**
(`02c6d7f952` + `ca7eb56e54`) while this profile was being landed. It wired the
resize-while-exported lease into **`molt-runtime`** (`molt_buffer_export` /
`molt_buffer_acquire` / `molt_buffer_release` + `ArrayBufferLease`) and left
`runtime/molt-cpython-abi/src/api/buffer.rs` **UNTOUCHED**. So the Phase-2 levers
below are now unblocked with no rebase-onto-interlock hazard in `buffer.rs`
itself — but any Phase-2 change must preserve the runtime-side lease interlock
(the export lifetime `owner`/lease is the resize-UAF guard) end-to-end.

Phase-2 plan, grounded in the numbers:

1. **Registry-mutex elimination (~41 ns + the multi-thread serialization
   cliff)** — the highest-value lever, but **NOT a naive "magic-header" swap.**
   Safety subtlety found while scoping it: the current
   `BUFFER_INTERNAL_REGISTRY` is a *deref-free set-membership* test on
   `view.internal`, and that is load-bearing — `PyBuffer_Release` /
   `descriptor_from_pybuffer` run on views whose `internal` was filled by a
   **foreign** C-extension `bf_getbuffer` (an exporter-private cookie that may be
   a small integer or point to unmapped memory). A self-describing box header
   that *dereferences* `view.internal` to read a magic word would be UB for those
   foreign internals. The CPython-aligned fix is to dispatch the release on
   **`view.obj`'s type** (native Molt buffer exporter → `Box::from_raw` ours;
   foreign → `bf_releasebuffer`), which needs neither a global lock nor a deref
   of `internal`. That is a careful refactor with direct Miri finding-C stakes
   (it touches the exact functions the provenance fix hardened) and should be its
   own focused, Miri-SB+TB-verified lane.
2. **Box pooling / inline (~46 ns)** — amortize the 1112 B `malloc`/`free`.
   Shrinking `MoltBufferView` is not free (`ndim` may reach 64; arbitrary
   strides are legal), so a thread-local free-list / arena of `BufferInternal`s
   is the safer shape.

Re-measure each against this baseline (`buffer_export_timing_profile`), update
`EXPECTED_ALLOCS_PER_EXPORT` in the same commit when the box is eliminated, and
keep `cargo test` + the Miri finding-C repros green under Stacked + Tree Borrows.

## LANDED addendum — BUFFER-DISTILL-55 measured before→after (2026-07-10)

Same machine, same session, `--release`, N = 200 000/variant. "Before" is
`origin/main` @ `3c82e4539d` re-profiled in a clean worktree immediately before
landing (the absolute ns numbers differ from the older table above — different
day/load — which is why the baseline was re-measured in-session).

| path | before (@3c82e4539d) | after | delta |
|---|---:|---:|---:|
| `PyBuffer_FillInfo` cycle | **179.9 ns**, 1 alloc / 1112 B | **6.6 ns**, **0 allocs** | **−96% ns, −100% allocs** |
| memoryview copy: OLD sub-cycle (`copy_pybuffer_for_memoryview`, box+registry, **no** object) | 274.9–285.4 ns, 1 alloc / 1112 B | — | — |
| memoryview copy: NEW **full** `PyMemoryView_FromBuffer` cycle (object construct + C reads + dealloc) | ≈ old sub-cycle **+ ~72 ns** object box (control A ≈ 71.7 ns) ⇒ ~350 ns equiv | **148.6–159.8 ns**, 1 alloc / 1144 B (the object itself) | **≈ −55% end-to-end; side box + registry node deleted (2→1 allocs)** |
| `PyMemoryView_FromMemory` full cycle | (old: object + 1112 B box + registry) | **77.0 ns**, 1 alloc | ≈ the bare object-allocation floor (control: 73.9 ns) |
| `PyObject_GetBuffer` internal | 1112 B box + registry insert/remove | right-sized `ExportInternal`: **32 B + 16 B/dim** (48 B @ 1-D), no registry | −96% bytes @ 1-D |
| ndim sweep (FromBuffer full cycle) | 277.2 / 277.4 / 280.4 / 286.7 ns (1–4-D, old sub-cycle) | 151.3 / 155.1 / 154.5 / 158.9 ns | still O(ndim)-flat |
| controls (same session) | A `Box<[u8;1112]>` 71.7 ns · B `Mutex<HashSet>` 81.3 ns | both paths **deleted** | — |

Multi-thread note: the registry was ONE global mutex serializing every
export/release across all threads; release now takes only the bridge lock
(already required for identity resolution on these paths), so the extra
process-wide serialization point is gone rather than made faster.

Gates (machine-checkable):

- `buffer_export_allocation_budget` — memoryview paths pinned at **1.0
  alloc/export = `size_of::<PyMemoryViewObject>()` (1144 B, storage embedded)**
  and FillInfo pinned at **0.0 allocs**; edited DOWN deliberately in the
  landing commit, cannot silently drift back up.
- `export_internal_is_right_sized` (`api::buffer::export_internal_tests`) —
  pins the GetBuffer internal at 32 B header + 16 B/dim.
- `test_memoryview_descriptor_outlives_constructing_frame`
  (`tests/test_object_protocol.rs`) — the anti-dangle gate: reads
  shape/strides/format AFTER the constructing frame returned (with a stack
  clobber), the exact UAF shape of the reverted `7da58cff8f` field-trick.
- Miri: lib + `test_object_protocol` + `test_modules` under **Stacked Borrows
  AND Tree Borrows** (`-Zmiri-ignore-leaks` for the documented immortal-global
  exception), 0 UB.

Residual (spec'd, not landed): true-zero-box `PyObject_GetBuffer` via the
cross-crate lease-lend — `molt_buffer_acquire` lending stable shape/strides
pointers out of the `ArrayBufferLease` that already outlives the view
(RuntimeHooks ABI change across hooks.rs / cpython_abi_hooks.rs / molt_api.rs).
That deletes the remaining 48 B/export allocation on the GetBuffer path; the
memoryview and FillInfo paths are already at their floor (one object / zero).
