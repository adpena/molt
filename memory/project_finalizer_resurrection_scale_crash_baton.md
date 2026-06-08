# Baton: pre-existing SIGSEGV on finalizer-resurrection at scale (LLVM/drops)

**Status (2026-06-08):** OPEN, P0-class. Discovered while verifying rung-1 task #56
(leak-gauge soundness under resurrection). **NOT caused by #56/#57** — proven
pre-existing by stash-isolation (see below). Out of scope for #56/#57; batoned
precisely.

## Symptom
A `__del__` that RESURRECTS its object (stashes `self` in a live container),
executed enough times, SIGSEGVs (exit -11 / 245) with NO stdout, on the LLVM
backend (drops live). Reliable, deterministic, very low threshold.

Minimal repro (`/tmp/rung1close/r_10.py` shape):
```python
_box = []
class R:
    def __del__(self):
        _box.append(self)          # resurrect: stash self
def run():
    n = 0
    while n < 10:                   # N>=~10 crashes; N=5 is clean
        x = R()
        del x                       # rc 0 -> __del__ -> resurrected into _box
        n = n + 1
run()
print("alive", len(_box))           # never reached on crash
```
Build: `molt build --target native --backend llvm --release`; run via
`safe_run.py --rss-mb 128 --timeout 15 -- <bin>`.

## Threshold / shape (measured)
- N=5  resurrections: **clean** (exit 0, "alive 5").
- N=10, 20, 50, 100, 1000, 5000, 30000: **SIGSEGV** (exit -11), no output.
- peak_rss at crash ~7 MiB → NOT an OOM, NOT stack-depth (100 is far too small).
- `_box.clear()` before exit (second-drop / true-free path) **also crashes**
  (N=50) → the fault is on the resurrected objects' subsequent free, not only at
  `_exit` teardown.
- Control: 30000 objects retained alive WITHOUT `__del__`/resurrection →
  **clean** (exit 0). So it is the resurrection path, not object count/teardown
  depth.
- CPython 3.14 handles every N fine ("alive N").

## Proven NOT a #56/#57 regression (stash-isolation)
With my `runtime/molt-runtime/src/object/mod.rs` change (#56 counter move)
STASHED to pristine HEAD and the runtime rebuilt, the SAME N=10 repro **still
SIGSEGVs**. The #56 change only reorders profiling-counter increments (no-ops when
profiling is disabled, which the crashing runs are) and adds a pure
`total_size_from_header_fields` local read; it does not alter the control flow
into `maybe_run_object_finalizer` or the free path. Decisive: pristine crashes
identically.

## Suspected locus (not yet root-caused)
`runtime/molt-runtime/src/object/mod.rs`:
- `maybe_run_object_finalizer` (~:1685) revive path: `inc_ref_bits(self)` →
  resolve+call `__del__` (which re-enters the allocator/GIL via
  `_box.append(self)`) → `fetch_sub(1)` (~:1760) → `prev > 1` ⇒ return true
  (resurrected). The re-entrant `__del__` runs allocator/list ops while we are
  mid-`dec_ref_ptr` of `self`; suspect a stale cached header field
  (`type_id`/`size_class`/`cold_idx`/`class_bits` read into locals in
  `dec_ref_ptr` BEFORE the finalizer) being used on the subsequent real free, OR
  the `HEADER_FLAG_FINALIZER_RAN`-set object's second drop hitting a class/cold-
  header that was reused/freed. The very low N threshold (≈10) suggests a
  cold-header-slab reuse / shared-cold-idx collision once a few resurrected
  instances accumulate, not a depth effect.
- Cross-check the round-12 drop-insertion + `finalizer_alloc_roots` path: the
  instance is `defines_del`, so it is heap-kept (good), but the resurrected
  object's later free may interact with `free_shared_cold_idx_for_class` /
  `bump_type_version` ordering.

## Why the rung-1 committed test does NOT hit it
`tests/differential/memory/finalizer_resurrection_leak_gauge.py` resurrects
EXACTLY ONE object once, then truly drops it (pop from `_box` + `del`). One
resurrection event is below the crash threshold; it is byte-identical to CPython
and leak-clean on native+LLVM. The #56 gauge fix is fully verified at that scale
(`dealloc_object=1` on both backends).

## Next steps
1. Reduce to the exact N where it flips (5→10) and bisect the slab/cold-header
   reuse with `MOLT_DEBUG_RC`/object tracing under `safe_run.py --rss-mb 256`.
2. Check whether native (drops dormant) also crashes at N=10 (value-tracking RC
   path) — if native is clean, the fault is LLVM-drop-lowering-specific; if both
   crash, it is in the shared `maybe_run_object_finalizer`/free machinery.
3. Add `tests/differential/memory/finalizer_resurrection_loop.py` (N≈50,
   resurrect-then-clear, bounded RSS, byte-identical) as the regression once
   fixed. Do NOT add it now (it would red the corpus on a pre-existing bug).
4. Structural fix + differential regression in the same change (infinite-loop/
   crash class = most severe; no runner-only workaround).
