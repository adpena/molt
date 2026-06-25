# String/Bytes Borrowed-View: Design Findings (DEPRIORITIZED — read before retrying)

Output of the `string-borrowed-view-design` orchestration (2026-06-25, 3 investigators +
synthesis). **`ready_to_implement = false`.** The perf-frontier survey called a borrowed-view
string Repr the highest-ROI node (~12 scoreboard cells). The design **falsified that** with
code evidence. Do NOT implement a borrowed-view string subsystem as a quick win.

## Falsifications (code-grounded)

1. **Scan ops are ALREADY zero-allocation.** `str.find/count/startswith/endswith`
   (`runtime/molt-runtime/src/object/ops_string.rs` ~L31/737/781/1051) operate on borrowed
   `&[u8]` slices via `bytes_find_impl`/`bytes_count_impl` (`strings.rs:14-61`) and return a
   scalar. `bench_str_find` does NO slicing. A view subsystem cannot speed these up — it is
   unmeasured perf theater. **Their RED (~0.68x) needs RE-ATTRIBUTION** (a different missing
   fact: per-call dispatch? UTF-8 width handling? measurement methodology?) — Phase-0 work,
   not a borrowed view.

2. **CSV is NOT view-eligible.** `csv.rs` `csv_parse_line` builds an owned `String` per field,
   but fields are TRANSFORMED (doubled-quote unescape, escapechar) — a borrowed view over a
   quoted field returns **WRONG bytes = silent-wrong-answer P0**. The real CSV win is a *byte*
   parser (replace `Vec<char>` accumulation with a byte scan), a DISTINCT optimization with no
   view dependency.

3. **str.replace / bytes.replace / memoryview.tobytes MUST allocate** (size change /
   scattered-buffer assembly) — not view-eligible. Classify MUST-ALLOCATE, not avoidable.

## If slicing views are ever pursued (memory-safety-CRITICAL)

- **P0 USE-AFTER-FREE / TYPE-CONFUSION:** 38+ sites across 13 files assume
  `object_type_id == TYPE_ID_STRING` implies inline bytes at `ptr+8`. A view type is UNSAFE to
  introduce until a single `resolve_string_bytes` authority is migrated EVERYWHERE and enforced
  by a disjointness test. A missed site reads a view's `{backing,offset,len}` struct as string
  bytes → OOB/arbitrary-memory deref. This is the resurrection/finalizer-class memory-corruption
  P0 the doctrine ranks ABOVE all perf work. This is why readiness is false.
- **Small-slice PESSIMIZATION:** a view costs `inc_ref(backing)` + header alloc + per-read
  indirection; for short substrings (the common Python case) it is SLOWER than the current
  inline copy and would turn green benchmarks red. Requires length-threshold gating
  (copy below N bytes, view above), justified by quiescent measurement.
- **REJECT the `TAG_STR_VIEW` + global `VIEW_POOL` design** (a draft spec proposed it): views
  must be ordinary `TAG_PTR` heap objects reusing the existing `MoltHeader` refcount + `[len][data]`
  layout. A side pool / parallel `StringBacking` allocator is a second source-of-truth for string
  memory (compound-interest-of-bugs trap).
- **Escape-completeness:** every escape point (hash/eq, dict/set keys, raw-bits-copy paths that
  bypass `dec_ref`) must be enumerated with a keep-vs-materialize disposition; one miss = dangling
  view = P0. WASM/native refcount parity (AtomicU32 vs Cell) must use the per-target primitive.

## Prioritization

Borrowed-view string work is **deprioritized** vs the loop-scalar-unbox keystone
(`keystone_checkedmul_loop_unbox.md`). Before ANY string-view work: (Phase 0) re-attribute the
scan-op REDs to their real missing fact via quiescent measurement — the survey's allocation
premise is false. The CSV byte-parser is a separate, view-free, safe optimization that can
proceed independently. Full design transcript: workflow `wf_a276cff0`.
