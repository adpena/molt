# Call anatomy: why compiled Python runs like an interpreter, and the architecture that fixes it

Status: accepted design, 2026-10-29, for ledger family J (runtime execution context), with I
(frames and exceptions), A (runtime-call ABI) and P (mid-end facts). Ledger rows: HF-185 (frame
cost, staged here), HF-561 (raise and catch) and the exception `__init__` descriptor row. Section 9
holds the programs and the method to reproduce every number.

This document is Molt's performance anchor. Read the stage status below before any performance
analysis, benchmark interpretation, work ordering or speed claim; the session digest prints its
summary line. A change that lands a stage or moves a measurement updates this table in the same
change.

## Stage status

Summary: 2026-11-09, release native, macOS arm64, user time against the managed CPython 3.12.16 (-I): Molt runs at 0.26x CPython (geomean of 27 benchmarks measurable on both sides); stages 1, 2, 3 and 5 are partial, 4, 6 and 7 not started.

| stage | state | landed | in flight | rows |
|---|---|---|---|---|
| 1 frame records | partial | per-statement line call removed on every backend (5014f2612f, 9c911e5ff8); the frame-record chain is the runtime's one frame authority (`frames/records.rs`, `ThreadRecord.frame_top`); a synchronous function pushes and pops its record inline on Cranelift, LLVM and WASM (`molt-codegen-abi` `frame_record`), and `molt_trace_enter_slot`, `molt_trace_exit` and `molt_frame_homes` are deleted | stateful activations inline (slice 2); no record for a leaf no observer can reach; exception-free home stores | HF-185, HF-261 |
| 2 thread-record state | partial | nested entry borrows custody, pending-exception byte inline (92f85fbfca, 821ca692c2) | attached execution context (lane hf198-attached-context) | HF-198, HF-205 |
| 3 compile-time binding | partial | direct runtime calls (2c9c0c1493); typed literal IDs (1752480285); literal-table design (1f72bfa18a); module global slot tables and compiled global slot reads and stores on Cranelift, LLVM and WASM (lane module-slots; section 4.5) | guarded call promotion design (HF-773 slice D); `module.attr` slot reads and the guarded builtin call (lane module-slots) | HF-507, HF-79, HF-200 |
| 4 inline caches | partial | method sites on Cranelift, LLVM and WASM: an inline class-version guard against a per-site record, a direct call of the recorded entry with up to two trailing defaults, `molt_load_method` and the thread-local method cache deleted (section 4.5, "Implementation (method sites)") | attribute and constructor sites; typed-layout deallocation | HF-202, HF-206 |
| 5 exceptions | partial | faster raise path (2c9c0c1493); normal try completion split from dispatch (87036f1ea6) | - | HF-561, HF-207 |
| 6 unboxed loop carriers | not started | - | - | HF-229 |
| 7 TIR inlining | not started | - | - | HF-773 |

Outside the call path, contained runtime-library costs dominate some of the worst cells (measured
2026-11-09 against the same baseline, symbolized `sample` profiles; each attribution checked against
the benchmark's source and a self-time breakdown by module):

- The cycle collector tracks containers in sharded, address-keyed `HashMap`s behind mutexes
  (`object/gc.rs`). Every container allocation and free takes a shard lock and a hash insert or
  remove (`gc_admit_membership`, `gc_untrack`). In `bench_struct`, a `Point(0, 0)` construction
  loop at 0.09x, the `gc` module holds 11% of self samples and mutex lock/unlock another 10%
  (callers not yet attributed); allocation and release hold about 30%. Every young collection
  copies the whole registry to filter by generation (`snapshot_registry`: most of `gc_pressure`,
  0.03x). HF-816 (b5e597ede) now keeps each generation's exact member list beside the registry,
  as CPython keeps one list per generation: `bench_gc_pressure` 12.8-13.6 s to 3.5-4.1 s
  (interleaved A/B, user time); the rest is the collection itself (HF-671). HF-816 phase 2 then
  replaced the registry with an intrusive GC head per container and per-runtime member lists
  (`generational_gc_authority.md`, "Membership representation"): track and untrack take no lock
  and no hash. Instructions retired, main ec4f533d1 to the lane, LLVM: `fannkuch_redux.py 9`
  -9.3%, `bench_struct` -6.0%, `bench_gc_pressure` -10.5% (user 4.34 s to 3.09 s); tracking
  frames fell from 12.2% to 4.0% of `fannkuch_redux` self samples. `bench_struct`'s remaining
  mutex samples come from `ClassLookupCache::lookup` and `runtime_callable_target_ptr`, not GC.
- `weakref_native` (0.05x) is not a registry cost: its symbolized profile spends 19% of self
  samples in native-call binding (`call::bind`) plus descriptor binding, thread-local reads and an
  allocation per call; the global weak-reference registry is under 4%. It belongs to stages 3 and 4.
  HF-817.
- The `struct` module re-parses its format and converts integers through the C-API bridge on
  every call (`molt-runtime-serial/src/structs.rs`; `struct.pack("<ii", i, i + 1)` costs 44,669
  instructions against 1,627, section 0); CPython caches compiled formats. HF-818.

The collector and `struct` costs need their own fixes and wait for no stage. Inside the call path the same profiles show frame
push and release (`take_invocation_namespace`, `FrameEntry::release`) at the top of method-heavy
code (stage 1) and traceback location decoding (`LocationRows`) at the top of `exception_heavy`
(stage 5).

## Why Molt is slow: five causes and their owners (2026-11-09)

1. Generated code is call-threaded: every operation enters the runtime. Stages 1-7 below.
2. Correctness and custody hardening made each runtime entry heavier before inline fast paths
   existed to skip it. Stages 2-5 (HF-198, HF-205).
3. Three backends hand-write the same fast paths, so each stage costs three implementations and
   the backends drift. Shared fast-path lowering, below (HF-821).
4. Effort went to process: in one measured week, 82 of 656 commits touched the compiler, runtime
   or differential tests. Family S owns presubmit cost (HF-801 and the consumer gate).
5. Nothing tracked performance per landing, so regressions such as the cycle collector's landed
   unseen. Family E (HF-222, HF-185, HF-616).

## Backend lowering: what is shared, what is not, and the order to fix it

The TIR, the SimpleIR passes, the module phase, the runtime, the refcount algorithm and the
generated ABI tables are shared. Op lowering is not: Cranelift (57k lines) and WASM (100k) lower a
SimpleIR copy rebuilt from TIR; LLVM (27k) lowers TIR. Each writes its own guarded call, NaN-box
integer helpers, truthiness, poll, exception check, frame enter and location store. On the
2026-11-09 survey LLVM had no guarded direct call and no tag-check integer lane, so it ran 17-28%
slower than Cranelift on calls (`bench_fib` 0.30 against 0.235 s, interleaved, release, macOS
arm64); the late stage now gives LLVM the integer lane (status below). Evidence:
logs/agents/backend-sharing-survey.md; design: logs/agents/shared-lowering/design.md; row HF-821.

End state (2026-11-09 re-plan, owner: engineer for the optimal end state, no transitional debt):
final typed TIR is every backend's input (doc 77 section B). A late lowering stage of TIR itself, in
the same graph, is the single place that chooses representation (stage 6) and expands every fast
path (stages 1-5) into typed primitive ops plus cold `CallRuntime(RuntimeFn)` edges, so SCCP and
value facts remove repeated tag tests and hoist invariant checks. Each backend only selects
instructions; NaN-box primitives come from one shared primitive generator in molt-codegen-abi. The
SimpleIR rebuild, the re-lift sites and the name-keyed representation plan that Cranelift and the
WASM op loop read are deleted as each backend moves.

Order: (0) typed runtime calls: one generated `RuntimeFn` per runtime entry (landed 3cbf168b9);
(A) land the finished slices; (B) design the late lowering stage of TIR (settled 2026-11-10);
(C) the integer lane through late TIR on LLVM and WASM, measured; (D) the remaining fast paths as
late-TIR expansions, deleting backend copies, with Cranelift moving to final TIR; (E) stages 4, 6
and 7 on TIR only.

**P0 (owner, 2026-11-11): the codegen foundation comes before more fast paths.** Since C1, the
late stage has gained fast paths for LLVM only (float, multiply), while Cranelift and WASM keep
their own copies; each optimization added a third and fourth implementation, and the copies
disagreed (Cranelift treated a stale pending-exception byte as an exception). No new fast path is
written per backend until the foundation below exists; optimization briefs are rescoped as
late-TIR expansions. The foundation is what rustc (`rustc_codegen_ssa`: one backend-generic walker
over a `BuilderMethods` trait that LLVM, Cranelift and GCC implement) and Zig (one IR; release and
WASM through LLVM) do:

1. Semantics once: every operation's meaning is a late-TIR expansion into the closed final-TIR
   primitive set. A runtime call is a cold edge, never the fast path; a name is never matched,
   and no new name-keyed runtime path is added (owner, 2026-11-11): globals read typed slots,
   attributes compile to shapes and slots.
   Representation: raw by default. A value whose type is proven stays a machine `i64`, `f64` or
   pointer in registers and is boxed only where it escapes to a place that must hold any type
   (an unknown callee, a heterogeneous container, the C API). Boxing costs tag tests, conversions
   and often a call on every use; the measure of a lowering is the instructions it leaves.
2. One codegen walker over final TIR, generic over a builder trait (word ops, loads and stores,
   blocks and branches, block parameters, calls by `RuntimeFn`). `WordBuilder` and `FrameBuilder`
   in molt-codegen-abi are its first parts. Cranelift and LLVM implement the trait and nothing else.
   Design and migration (W1 walker, W2 LLVM families, W3 Cranelift switch):
   [codegen_walker.md](codegen_walker.md).
3. WASM through LLVM's `wasm32` target from the same walker, if a measured spike holds (speed, size,
   build latency, split runtime, attached context, exception handling); molt-backend-wasm then
   keeps host, link and ABI glue, and its op lowerings are deleted, not migrated.
4. Test stubs of the runtime are generated from `RuntimeFn` or are the real runtime; hand-written
   stub runtimes in test strings copy entry names and drift (HF-831).

Foundation order, by duplicate code each step deletes: delete WASM `lir_fast` (10,246 lines,
admitted only by a function-name substring no producer emits); the LLVM `wasm32` spike; the
builder trait and walker with LLVM as its first implementation; Cranelift onto final TIR. Both
native backends already call runtime entries only by typed `RuntimeFn`, decoded once at the TIR
lift ([typed runtime imports](typed_runtime_imports.md), 2026-11-12). Measured reason the order matters (2026-11-11, LLVM release,
macOS arm64, instructions retired, managed CPython 3.12.16 -I): the worst gaps are fannkuch 8.2x
CPython (27.0 G excess; below CPython's count since HF-837, below), `bench_method_defaults` 2.9x (16.4 G), `bench_math_numeric` 5.6x (6.8 G),
mandelbrot 4.7x (6.6 G), `bench_dict` and `bench_list` 2.3x; their buckets (object lifecycle,
container access) are fast paths every backend needs, so each is written once on the foundation.

Status (2026-11-10): C1 landed (229180b0b): one set of NaN-box word generators in molt-codegen-abi
(`word.rs`, `boxed_word.rs`), LLVM's copies deleted. C2 landed (077d6de6b): the stage
(`run_late_lowering`, spec 0100) expands integer `+`/`-` on boxed words for LLVM; LLVM's seven
string runtime-name matches for numeric fallbacks are deleted. `bench_fib`, LLVM release, macOS
arm64: 8.77 to 8.53 billion instructions retired (-2.7%, VERIFIED, three interleaved runs per
binary, output identical); molt_diff on 13 integer programs passes native 13/13, LLVM 13/13, WASM
12/13 (the one failure was HF-826, fixed on main). C3 landed (8763d0b60): truthiness of bool and
inline-int words without a runtime call. C2b and C4 (HF-821 lane): a boxed operand with a literal
(`n - 1`) tests only the boxed word; `==`, `!=`, `<`, `<=`, `>`, `>=` compare inline payloads; the
fifth primitive `BoxInlineInt` boxes a window value without raising; the stage now runs after the
terminal type refinement, so an expanded int keeps its `I64` type. `bench_fib`, LLVM release,
macOS arm64, against C3: 8.40 to 7.57 billion instructions retired (-9.9%, VERIFIED, three
interleaved runs per binary, output identical); user time 0.25 to 0.23 s, below Cranelift's 0.235 s
(row above). Next: C5 `Mul` with a cheap exact test; list index and store fast paths (fannkuch's hot
path); WASM moves once the stage covers its lane parity checklist. C6 (2026-11-12): mixed int and float
operands take two arms that convert the inline-int payload exactly (`IntToFloat`), and true
division (`Div`) computes in F64 on every arm; two ints divided by zero leave for the runtime,
which raises the int message. `spectral_norm` (whose `eval_A` divides ints and adds a float and an
int), LLVM release, macOS arm64, instructions retired, min of 2: 11.50 to 8.27 billion (-28.1%,
output identical), below Cranelift's 9.58 billion; `bench_float`, `nbody` and `mandelbrot` within
0.13%. Cranelift keeps its backend-local mixed paths until it consumes final TIR.

fannkuch list lifecycle (2026-11-12, HF-837). The 8.2x gap was the runtime, not the call path: a
symbolized profile of `fannkuch_redux.py 10` on main (LLVM release, 9,165 self samples) held list
destroy 27.0%, allocation 17.4%, slicing 12.4%, index get/set 8.6%, range and list iteration
6.6%, GC membership 4.1% and compiled code 1.6%. The general causes are fixed once for every
backend:

- Destroy: a terminal release built an edge sink, counted its capacity by walking the payload,
  and detached every item, for a list of inline ints with nothing to release.
  `heap_lifecycle::retires_without_edges` proves the empty case from the header (no C view, weak
  reference, scheduler retain, sidecar or owned class edge) and one tag test per item of a list,
  tuple, slice or range; such an object frees its storage directly.
- Slicing: every slice bound became a `BigInt` (`DecodedSlice`); an inline int now stays an
  `i64`. A list slice copied the whole list, built an index vector, then copied again;
  `seq_access::snapshot_slice` copies only the selected items into the new list's own backing. An
  equal-length slice store of non-heap words overwrites in place.
- Index get/set: `index_impl` and `store_index_impl` serve the exact builtin containers first, on
  one header load: an exact list at an inline int in range is one locked bounds check and load or
  one item replacement, and an exact dict goes straight to its table.
- Parallel assignment `a, b = x, y` no longer builds and unpacks a tuple: the values evaluate left
  to right, then the targets, as CPython compiles it.

Release, macOS arm64, instructions retired and user time, minimum of 7 interleaved runs against
main 49242cd306 built in its own worktree, output identical: `fannkuch_redux.py 9` LLVM 27.75 G to
18.77 G (-32.4%, 0.95 to 0.63 s), Cranelift 28.03 G to 19.14 G (-31.7%, 0.96 to 0.65 s); managed
CPython 3.12.16 -I runs it in 3.97 G and 0.16 s. `bench_list` LLVM -7.4%, Cranelift -5.6%;
`bench_dict` LLVM 0.0%, Cranelift +1.0%. The same profile on the lane (LLVM, 5,995 self samples
against 9,165): list destroy 18.0%, allocation 18.9%, slicing 11.8%, index get/set 10.1%,
iteration 9.9%, GC membership 6.0%, compiled code 2.3%.

The flip `perm[:k + 1] = perm[k::-1]` is the TIR pass `slice_reverse` (after SCCP, every backend):
a `StoreIndex` whose value is an `Index` of the same SSA list by a step -1 slice becomes one
in-place reverse when the bounds name one range. Each bound decomposes into a base value plus a
constant offset through copies and `Add`/`Sub` of int constants, and the target's offset must be
the source's plus one (or both None), so `seq[:k + 1] = seq[k::-1]`, `n = k + 1; seq[:n] =
seq[n - 1::-1]`, `seq[i:j] = seq[j - 1:i - 1:-1]` and `seq[:] = seq[::-1]` qualify and a near
miss does not. The pass emits `list_reverse_slice(seq, start base, offset, stop base, offset,
shape)`, a shared boxed runtime call every backend lowers from the generated table, before the
statement's chain of blocks: the kernel reverses in place for an exact list without a C view whose
bases are inline ints and whose bounds are `>= 0`, and otherwise declines to the statement as
written. A shape whose proof is missing is reported under `MOLT_SLICE_REVERSE_REPORT=1`. Release,
macOS arm64, minimum of 7 interleaved runs against main 388c6de982 in its own worktree, output
identical: `fannkuch_redux.py 9` LLVM 18.76 G to 3.63 G (-80.7%, 0.65 to 0.12 s), Cranelift
19.13 G to 4.09 G (-78.6%, 0.67 to 0.14 s), below CPython's 3.97 G and 0.16 s; `bench_list` and
`bench_dict` within 0.3%. Profile (LLVM, 1,216 self samples against 9,165 on ee6d276030): list
destroy 11.9%, allocation 16.4%, slicing and the in-place reverse 14.9%, index get/set 20.0%,
iteration 1.5%, GC membership 3.4%, compiled code 9.8%. What remains is the call-threaded shape:
every list index is a runtime call on LLVM and WASM (Cranelift keeps a backend-local fast path),
`for r in rxrange` iterates a range object through the runtime, and `perm1[:]` allocates a list per
permutation; these move to the codegen foundation as compiled list and range primitives.

P0, WASM through LLVM's wasm32 target (spike 2026-11-11, decision: yes, after four blockers).
LLVM's wasm32 target compiles final TIR into a relocatable object that wasm-ld links against
the runtime's relocatable module with no import signature mismatch. On Node, all five spike
programs print CPython 3.12.16's output. Against the WASM backend (same IR, runtime, wasm-ld
flags and wasm-opt pipeline; interleaved medians; macOS arm64): `bench_fib` 0.68 to 0.39 s and
17.0 to 10.4 billion instructions retired, `bench_calls` -16% and `bench_sum` -18% instructions,
nbody 200000 steps 1.65 to 1.48 s; startup-bound runs are equal. Costs: linked size +12% on
small programs and +72% on nbody, and backend compile 22 s against 0.6 s (nbody 250 s against
6 s). Native LLVM takes 11 s on the same IR, so compile cost belongs to the LLVM lane, and
wasm32 object emission is about 2x aarch64's. Blockers, in order: (1) the wasm32 runtime maps
callable keys to an app-filled reserved table region, which wasm-ld cannot fill; resolving keys
through the runtime's own table, as native does, fixes it, and the WASM backend passes on that
runtime too (HF-699 K1 remainder); (2) `tools/wasm_link_*.py` edits the WASM backend's object
shape and must admit a standard object; (3) split runtime needs a PIC side-module link (LLVM uses
the shadow stack) and the `molt_runtime` import namespace; (4) compile latency needs the stdlib
object cache and parallel module codegen. Then `--target wasm` moves to LLVM and the WASM op
lowering, the reserved region, the app-owned `molt_call_indirectN` and `molt_isolate_import`
and the object surgery in the link tools are deleted. Evidence:
logs/agents/lane-llvm-wasm32-spike/notes.md; prototype commit 47b9c06ac3 (branch
proto/llvm-wasm32-spike in worktree slot-02, not landed).

## Whole-pipeline structure (mapped 2026-11-10)

The backend view above understates the problem. One function passes through these stages
(VERIFIED by file:line reading; the evidence map is in logs/agents/pipeline-map/):

    Python: discover (parse 1) -> parse 2 -> sema -> visit -> Python mid-end -> serialize to
      SimpleIR (identity = variable-name strings) -> msgpack lease
    Rust:   9 SimpleIR passes -> lift to TIR -> 27 TIR passes -> module phase ->
      project back to SimpleIR (Cranelift, WASM, Luau) | TIR (LLVM) | SimpleIR only, no TIR (Rust)

Structural defects, each with its owner:
1. Three optimizers for one fact family: the Python mid-end, nine Rust SimpleIR passes and the
   TIR passes. Constants fold in four places, check_exception is removed in three, megafunctions
   split in Python and twice in Rust. Owner HF-773 (retire the Python mid-end, then the SimpleIR
   passes into TIR).
2. Round trips instead of one IR: TIR is lowered back to SimpleIR at four sites and re-lifted at
   three; the representation plan lifts again and reruns the whole TIR pipeline on a clone
   (representation_plan.rs:360-383), so a function with a try block is optimized twice. Owner
   HF-821: final TIR as every backend's input deletes the projections.
3. Identity spelled as strings: a value is its variable name on the wire and in the Python
   mid-end; TIR ops carry `_simple_out`, `_var` and `_type_hint` string attrs; the representation
   plan is keyed by name and drops a fact when two values share one; the WASM LIR lane is chosen by
   a function-name substring; Cranelift names 380 runtime entries by string (329 already in
   RuntimeFn). Owners: HF-822 (front end emits TIR with typed ids), HF-821 (backends on final TIR).
4. Build choices do not reach codegen: `TargetInfo.profile` is always ReleaseFast
   (target_info.rs:349); LLVM is always O3; the Cranelift opt-level branch that the profile should
   select can never run; `molt run` defaults to dev while `molt build` defaults to release; choices
   cross into Rust as environment variables (757 of 765 registered variables affect output). Owner:
   a typed build plan carried to every consumer (new row).
5. Refusals are late: native, LLVM and WASM panic on an unsupported op; only Luau and Rust use the
   early, diagnosed target admission. Owner: one admission for every backend (new row).
6. Facts lost between stages: Python's exact_class is never serialized; escape analysis's
   stack_eligible is set before the lift and the Cranelift reader sees the projected op (INFERRED
   lost; verify). Owners: HF-822 and HF-821 remove the stage boundaries that drop them.

What an Astral-grade design does instead, and where Molt is going: parse once (Ruff), one semantic
model (ty), emit typed IR with dense typed ids, optimize once in Rust, and lower once to machine
code without a round trip. Every row above retires a piece of the current chain; none adds a
stage.

## 0. Summary

**Finding.** Molt's native code is a call-threaded interpreter. A compiled function is a sequence
of calls into the Rust runtime. Each call re-derives the execution context from thread-locals and
does one Python-level bookkeeping step: push a frame, store a local into a frame home, set the
line, set the column, look up a global by its string name, admit a call, charge the recursion
counter, poll for async work, change a reference count. CPython 3.12 does the same steps inside one
function with the thread state in a local variable, frames on a bump-allocated data stack, and
caches inside the bytecode. So Molt pays more per Python-level step than the interpreter it should
replace.

**Headline numbers** (macOS arm64, release profile, origin/main `fa784d0c77`, CPython 3.12.15 on
the same host; instructions retired per unit with startup subtracted):

| unit | Molt | CPython | Molt / CPython instructions | wall speed vs CPython |
|---|---:|---:|---:|---:|
| Python call, `total = add(total, i)` | 3,623 | 866 | 4.2x | 0.24x |
| `fib(30)`, per call | 4,346 | 520 | 8.4x | 0.15x |
| `for i in range: total += i`, per iteration (inline int) | 361 | 521 | 0.7x | 1.25x |
| same loop when `total` exceeds 47 bits (heap int) | 2,377 | 521 | 4.6x | 0.20x |
| `p.x = i; total += p.x`, per iteration | 1,736 | 614 | 2.8x | 0.38x |
| `total = c.inc(total)` method call | 4,697 | 832 | 5.6x | 0.18x |
| `Point(0, 0)` + 2 stores (perf-gate `struct`) | 27,001 | 2,474 | 10.9x | 0.08x |
| `struct.pack("<ii", i, i + 1)` | 44,669 | 1,627 | 27.5x | 0.026x |
| `raise ValueError("x")` caught | 210,274 | 2,038 | 103x | 0.006x |

The one loop where Molt wins (inline-int sum) still costs 361 instructions for one add. A
compiled loop needs about 4.

**Top three structural causes.**

1. **Frames are eager and runtime-owned.** Every compiled function pushes a `FrameEntry` onto a
   thread-local `RefCell<Vec>`, retains code, globals and builtins, takes an invocation namespace,
   borrows a homes array, sets an argument-zero context, mirrors each local into its home, and
   writes the line and column through runtime calls. In the callee `add` that is 11 runtime calls
   per invocation. Frame push and pop (`molt_trace_enter_slot` 17.7%, `molt_trace_exit` 17.9%,
   `molt_frame_context_set` 4.1%, `molt_frame_homes` 2.5%) plus locations (`molt_frame_set_col`,
   `molt_trace_set_line`, `molt_frame_set_line_col`, 8.1%) take 51% of the call loop. In `fib` the
   same group takes 48%. CPython pushes a `_PyInterpreterFrame` with a pointer bump and about ten
   stores (`Include/internal/pycore_frame.h:_PyFrame_PushUnchecked`) and creates a frame object only
   when code observes it (`Python/frame.c:_PyFrame_MakeAndSetFrameObject`).
2. **Every name is bound late, through the runtime, by its spelling.** A call to a module function
   reads the global by a string-keyed dictionary lookup on each call (`molt_module_get_global`:
   11.7% of the call loop, 10.6% of `fib`). A method call hashes the method name's bytes on each
   call (`molt_load_method`, 24% of the method loop). An attribute get or set is a runtime call that
   recomputes the object's shape id before it reads its own cache (72% of the attribute loop). A
   constructor resolves `__init__` through the MRO and allocates a bound method on each
   construction (50% of the `struct` loop). `ValueError("x")` resolves `__init__` to a CPython-ABI
   slot wrapper and round-trips through the C-API bridge (62% of the raise loop). CPython
   specializes each such site to a version guard and an index
   (`Python/bytecodes.c:LOAD_GLOBAL_MODULE`, `LOAD_ATTR_INSTANCE_VALUE`, `CALL_PY_EXACT_ARGS`).
3. **Compiled code holds no execution context, so every helper re-enters the runtime.** Safepoint
   polls (`molt_async_work_poll_and_exception_pending`) are runtime calls at every loop backedge and
   call return: 44% of the inline-int loop. Reference counts are out-of-line calls, also on
   immediate ints that need no count: 23% of the same loop. Call admission
   (`molt_function_direct_call_eligible`, `molt_handle_resolve`, `molt_recursion_enter_fast`/
   `exit_fast`) is four calls per direct call. `_tlv_get_addr` is 8.7% self time in the call loop and
   9.6% in `fib`. CPython reads one `eval_breaker` word (`Python/ceval_macros.h:CHECK_EVAL_BREAKER`)
   and passes `tstate` as an argument.

**Plan in brief** (section 5). Seven migrations, each landable alone, each deleting the lane it
replaces:

| stage | migration | closes or moves | target |
|---|---|---|---|
| 1 | Frame records: native-stack frames, plain-store locations, lazy frame objects | HF-185 (frame cost 2, 3, 6), HF-261, part of HF-229 | call loop 3,623 → ≤ 1,900 instr; `fib` 4,346 → ≤ 2,300 |
| 2 | Thread-record state: eval-breaker word, inline recursion depth, inline call admission, inline refcount | HF-185 (stages 4, 5), HF-205, HF-198 (part) | inline-int loop 361 → ≤ 130; call loop → ≤ 1,000 |
| 3 | Compile-time binding: global slots, native signatures, direct calls, context argument, literal table | HF-198 (stage 2), HF-507, HF-79, HF-200 | call loop → ≤ 250; `fib` → ≤ 300 |
| 4 | Inline caches in compiled code (attributes, methods, constructors, module attributes, typed names) and typed-layout deallocation | HF-185 (attr, `struct`), HF-202, HF-206 (part) | attr 1,736 → ≤ 150; method 4,697 → ≤ 350; `struct` 27,001 → ≤ 600 |
| 5 | Exceptions: native exception classes, lazy tracebacks, class-handle matching | HF-561, HF-207, new row (C-API detour) | raise 210,274 → ≤ 1,500 |
| 6 | Unboxed loop carriers and observation-point locals | HF-185 (loop half), HF-229 | inline-int loop → ≤ 8; call loop → ≤ 80; attr → ≤ 40 |
| 7 | TIR inlining of proven small callees with inline frames in the location table | new | call loop → ≤ 12 |

A stage's target assumes the stages it depends on have landed. The end state is 10-80 instructions
for a Python call and single-digit instructions for a scalar loop iteration: 45-300x below today on
every row, and 5-70x below CPython. Stages 1, 4 and 5 touch disjoint authorities and can run in
parallel lanes; stages 2 and 3 follow stage 1; stage 7 follows stage 3.

## 1. Method and environment

- Host: macOS 26.4 (Darwin 25.4.0), Apple arm64, 18 cores, load average 11-13 during the runs
  (other lanes were building). Instructions retired and cycles come from `/usr/bin/time -l` and do
  not depend on load. Wall times are medians of 5 runs (3 for the 15 s raise run).
- Molt: a managed development image whose compiler, frontend, backend and runtime equal origin/main
  `fa784d0c77` (`git diff origin/main HEAD -- runtime src/molt/frontend src/molt/cli` is empty). Build: `bin/python -B -m molt.cli build --build-profile
  release` with `MOLT_KEEP_SYMBOLS=1`, Cranelift native backend.
  Binaries are 14.0 MB (23.2 MB with `import struct`).
- CPython: the same image's interpreter, 3.12.15 (clang 22.1.3), run as `python -B -I -S prog.py`.
- Startup subtracted: Molt empty program 192.8M instructions; CPython `-c pass` 88.7M.
- Per-unit instruction counts divide the run's instructions minus startup by the number of units.
  Small-N differential runs (1,000 vs 2,000 iterations) agree for the expensive rows (raise 215K,
  `struct` 26.7K) and are too noisy for cheap rows, so the large-N numbers are authoritative.
- Anatomy: `tools/opt_explorer.py anatomy` (static instructions, branches and call sites per
  function and loop), `objdump` with resolved callees (`disasm_sym.py`), and a sampler
  (`profile_calls.py`) that charges each `sample` call-graph node whose parent is compiled code to
  that runtime entry point (inclusive time of each runtime call that compiled code makes).
- IR: `molt debug ir <source.py> --format json` (the frontend serialization boundary;
  backends lift this wire form into typed SSA before optimization).

Dynamic per-call-site counts with `lldb` were blocked: a launched process did not progress within
120 s (likely a debugger authorization prompt on this host), so runtime calls per unit come from reading the hot
path in the disassembly, confirmed by the sampler's per-callee shares.

## 2. Measured anatomy

### 2.1 One Python call: `total = add(total, i)` (p8)

Source: `def add(a, b): return a + b`, called 10M times from a `for` loop in `main()`.
3,623 instructions per iteration; CPython 866. The loop body without the call (p2) costs 361, so
**the call itself costs about 3,260 instructions in Molt and about 345 in CPython (9.4x).**

The frontend emits this SimpleIR for `add` (20 operations; `ir/p8_call.txt`):

```
trace_enter_slot; check_exception
frame_home_store a -> home[0]; check_exception
frame_home_store b -> home[1]; check_exception
const_none; const 3; frame_context_set(None, 3, None)
line 2
add v56, v57; check_exception
trace_exit; ret
(two cold labels: trace_exit; ret_void)
```

Native code for `add` is 248 instructions with 19 call sites. Its non-raising path executes 137
instructions and makes 11 runtime calls (disassembly with resolved callees). The integer add itself
is inline: tag test, 47-bit sign extension, add, range check, retag, about 15 instructions.

| # | runtime call in `add` (hot path) | what it does | why it is there | cost share of loop |
|---|---|---|---|---|
| 1 | `molt_thread_record` | returns the thread record pointer from TLS | compiled code has no context argument | 1.0% |
| 2 | `molt_trace_enter_slot(code_id)` | enters runtime; takes the invocation namespace; pushes a `FrameEntry` on `FRAME_STACK` (TLS `RefCell<Vec>`), retains code, globals, builtins; pushes `TRACE_FRAME_PUSH_STACK`; sizes and lends homes from the code slot's frame plan | eager frame push | 17.7% |
| 3 | `molt_frame_homes(2)` | enters runtime; checks pending exception; returns the top entry's homes pointer | locals live in runtime homes for introspection | 2.5% |
| 4 | `molt_frame_context_set(None, 3, None)` | enters runtime; decodes argument-zero kind; writes the frame's Python context | zero-argument `super()` support, emitted in every function | 4.1% |
| 5 | `molt_frame_set_col(4, 16)` | TLS `FRAME_STACK` borrow; writes col/end_col of the top entry | caret positions in tracebacks | 3.5% (all four `set_col`) |
| 6 | `molt_trace_set_line(2)` | enters runtime; checks two trace flags; writes the line of the top entry | traceback line | 3.3% |
| 7 | `molt_frame_set_line_col(2, 4, 16)` | writes line and columns again | same | 1.3% |
| 8 | `molt_frame_set_col(11, 16)` | column of the `+` | same | (in 5) |
| 9-10 | `molt_frame_set_col(4, 16)` x2 after the add | as above, twice with equal values | column after the add and again before `trace_exit` | (in 5) |
| 11 | `molt_trace_exit` | enters runtime; pops `TRACE_FRAME_PUSH_STACK`; pops and releases the `FrameEntry` (`FrameEntry::release`, `release_parts`, `frame_bindings_exit`) | eager frame pop | 17.9% |
| - | `frame_home_store` x2 | inline: load old home, store tag and value, `dec_ref` the old value if owned (not taken here) | mirror locals into homes | in compiled self |
| - | 4 `check_exception` | inline: load the pending byte through the record pointer, branch | after operations that the effect oracle treats as may-throw, including the home stores | in compiled self |

The caller loop is 141 static instructions in 18 blocks. Its hot path executes 119 instructions and
13 runtime calls per iteration, plus the direct call of `add` (`molt_call_func_owned` is the cold
fallback; two `molt_dec_ref_obj` sites run only when an old home value is owned):

| runtime call in the caller loop | what it does | why it is there | share |
|---|---|---|---|
| `molt_module_get_global(None, "add")` | enters runtime; reads the active globals from `FRAME_STACK`; checks a trace flag; string-keyed dict lookup in globals, then builtins | the global binding is read by name every call | 11.7% |
| `molt_function_direct_call_eligible(f, 2, shape)` | enters runtime; checks type, arity, binder flag, execution kind | call admission is a runtime query | 3.3% |
| `molt_handle_resolve(f)` | decodes the boxed handle to a pointer | to load the code pointer to compare with the static target | 0.5% |
| `molt_recursion_enter_fast` / `molt_recursion_exit_fast` | TLS depth charge and release | Python recursion limit | 2.7% |
| `molt_async_work_poll_and_exception_pending` x2 | enters runtime; `service_async_work` (signals, pending GC, retirement); reads pending exception | eval-breaker poll at the call return and at the backedge | 8.6% |
| `molt_inc_ref_obj` x3, `molt_dec_ref_obj` x3 | out-of-line; return at once for immediate ints | no type fact elides counts on immediates | 8.9% (with callee) |
| `const_str "add"` | heap literal handle for the global name | name-keyed global read | small (cached) |

Remaining shares: compiled code self time 8.3%, `_tlv_get_addr` is spread across the entries
above (8.7% self). Summed: frame push and pop 43.2%, global lookup 11.7%, polls 8.6%, counts 8.9%,
locations 8.1%, call admission 6.5%, compiled code 8.3%.

**A Python call makes 24 runtime calls (13 in the caller, 11 in the callee); 9 of them take the
full runtime entry** (`RuntimeExecutionGuard::enter`), the rest touch one or more thread-locals.
Compiled code executes 256 of the 3,623 instructions per iteration (119 caller, 137 callee); **93%
of the instructions of a Python call run inside the runtime.** CPython's `CALL_PY_EXACT_ARGS`
performs five `DEOPT_IF` tests, a frame push and argument copies, with no calls
(`Python/bytecodes.c:2761`).

### 2.2 `fib(30)` (p1)

4,346 instructions per call; CPython 520. `fib` compiles to 812 instructions with 89 call sites
(`fib` is not inside a loop, so the backend emits a `molt_frame_set_col` call before every
operation that carries a column: 30 sites). Inclusive shares (1,298 samples of `fib(33)`):
frame push/pop/context/homes 29.9%, location writes 18.2% (`molt_frame_set_col` alone 13.1%),
`molt_module_get_global` for the two recursive reads of `fib` 10.6%, `molt_string_from_bytes` for
the `"fib"` name literal 3.3% (HF-507: the literal is rebuilt per call), counts 6.3%, polls 2.7%,
admission 3.8%, `molt_is_truthy` 2.1%, compiled code 10.2%.

HF-185 recorded `fib` at 0.01x of CPython on 2026-10-14. It is 0.15x now: the landed fixes removed
the run-time name resolution and the 56 binding checks, but not the frame model.

### 2.3 Counted loops (p2, q2b, q2)

`for i in range(N): total += i` in a function: 361 instructions per iteration while `total` fits
47 bits; 2,377 when it does not. CPython 521 in both regimes (it allocates a 2-digit int each
iteration).

The loop body is 104 static instructions with 8 call sites. The SimpleIR per iteration has 20
operations: `frame_home_store` for `i` and for `total`, five `check_exception`, `inplace_add`,
`loop_index_next`. Shares on the inline regime (2,179 samples): the backedge poll 44.0%
(`service_async_work` 13.1% self, `runtime_state_for_gil_in` 7.4%), `molt_dec_ref_obj` and
`molt_inc_ref_obj` on immediate ints 23.2%, compiled code 24.3% (home stores and checks).

In the heap-int regime the add takes the runtime path: `molt_inplace_add` 30.2%, and freeing each
dead int costs `dec_ref_ptr_terminal` 33.7% because a leaf int runs the generic detach cascade
(`DetachedEdgeSink`, `visit_common_payload_owned_values`, `clear_cycle_edges_with_sink`).

### 2.4 Attribute, method, constructor, module function

- **Attribute (q3b, inline regime):** `molt_get_attr_object_ic` 42.3% and `molt_set_attr_object_ic`
  30.3% of the loop. Inside them, the cache check recomputes identity: `object_shape_id` 11.3%,
  `instance_dict_bits_ptr` 8.7%, `class_instance_shape_id` 7.4%, `object_has_class_shape` 7.1%.
  The per-site field cache from ae016af96 exists, but it lives behind a call and its key is
  recomputed by four calls. CPython: `LOAD_ATTR_INSTANCE_VALUE` compares `tp_version_tag` and reads
  `values[index]`.
- **Method (q4):** `molt_call_loaded_method1` 58.6% (frame push/pop again inside),
  `molt_load_method` 23.7%. `molt_load_method` takes the name as `(ptr, len)` bytes, so each call
  hashes and validates the name (`hash_string_bytes`, `from_utf8`) and pins an IC entry.
  Method sites (2026-11-12, section 4.5) replaced both runtime entries on the hit path: p4 runs
  at 352 instructions per call on LLVM against 2,046 before, and a symbolized profile of
  `bench_method_defaults` shows no method load, call dispatch or method cache.
- **Constructor (p5, the perf-gate `struct` row):** `call_type_with_arguments` 51.6%:
  `construct_regular_class` looks up `__init__` through the MRO (`class_namespace_lookup_mro`
  12.3%), allocates a bound method per construction (`molt_bound_method_new` 10.4%), allocates
  the instance with GC admission (`gc_admit_membership` 5.2%, then a registry shard mutex; HF-816
  phase 2 removed the mutex and the hash). Freeing
  the instance costs 31.3% (`dec_ref_ptr_terminal`, field visits through pinned tuples). The
  `__init__` body is 11.7%.
- **`struct.pack` (p6):** `struct.pack` is a compiled Python wrapper (`struct__pack`) around the
  `molt_struct_pack` intrinsic. Per call: `molt_load_method` on the module 22.2% (it walks the
  module class's MRO before the module dict: `class_namespace_lookup_mro`), runtime argument
  binding and frame push for the wrapper (`bind_python_frame` 10.4%, `frame_stack_pop` 6.7%),
  `_normalize_format` 11.0%, the intrinsic 16.2%, of which packing proper is 5.7%.

### 2.4.1 Construction argument storage (HF-685, 2026-11-04)

The later canonical `plain_construction`, `new_and_update` and
`dataclass_construction` programs distinguish a builder cost from the broader
constructor/namespace gap. The cell below compares source `ca1cdefc68c1` with
`540e467c5d`, Linux x86_64, Cranelift, logical release, release compiler and
release-output runtime, Rust 1.99.1 and Valgrind 3.22.0. It counts Callgrind
instruction reads (`Ir`); hardware retired instructions are a separate cell.

Compile each program from `tools/perf_instructions.py` once with
`MOLT_KEEP_SYMBOLS=1`. Capture that same executable with Callgrind at N=0 and
10,000/20,000 actual operations (`new_and_update` receives N=100,000/200,000).
Subtract N=0 function costs and divide by operations; loop/control work stays in
the slope. Every raw exclusive sum matches its event summary. All six K/2K slopes
agree within 0.1%; all 18 outputs and program hashes match exact CPython 3.12.16.
The frontend image is CPython 3.12.15. Function costs follow the
[Callgrind format](https://valgrind.org/docs/manual/cl-format.html);
inclusive rows overlap and cannot be summed as a semantic partition.

| Primitive | Base Ir/op | Head Ir/op | Change |
| --- | ---: | ---: | ---: |
| `plain_construction` | 16012.50910 | 12123.85650 | -24.285% |
| `new_and_update` | 27740.20570 | 27875.10090 | +0.486% |
| `dataclass_construction` | 29268.72200 | 29247.29260 | -0.073% |

Plain construction executes 17 pointer hashes per operation. Its builder map is
checked twice per push, then an independent storage set is checked again; both
registries are updated on allocation and retirement. Hash bodies plus the three
provenance helper bodies account for 3,435.96 Ir/op (21.46%), before Box and lock
overhead. This is a measured removable subset, not a complete mechanism partition.

The managed builder now contains its typed `CallArgs` value inline. It has one
storage owner, with Rust size/alignment and the `inline_rust`,
`linear_unpublished` heap-kind contract. The external Box, pointer payload,
builder map, storage set and runtime-state mutex are retired together. A live
compiler-owned builder supplies the unsafe projection's lifetime; a live
wrong-kind object is rejected before projection. This retains the existing ABI
precondition rather than admitting forged/freed addresses. Native, LLVM and WASM
pass opaque handles and contain no payload offset. Shared builders retain copies;
unique builders transfer argument edges. Cleanup empties both fields before any
edge release, leaving valid empty inline storage through deallocation. Allocation
refusal releases prepared keyword/vector storage; inline bytes belong to normal
object accounting. The owning call-custody contract is
[SIMPLE_IR_JSON_SCHEMA.md](../../spec/areas/compiler/SIMPLE_IR_JSON_SCHEMA.md).

The corrected mechanism removes every builder-registry hash caller. Plain's
remaining usize-hash slope is 0.0003 calls/op; its mimalloc allocation entry falls
from 4.0000 to 3.0003 calls/op, and free from 4.0005 to 3.0007, matching the removed
external Box. The other primitives have zero steady-state builder calls:
new-and-update's four pointer hashes belong to attribute IC lookup, while the
dataclass's nine belong to retirement and callable-target lookup. Their larger
resolution/retirement obligations remain; these measurements do not close HF-685,
HF-680/HF-688, HF-205 or HF-79/HF-226.

Two sequential read-only design and implementation-readiness reviews accepted
this mechanism after an explicit producer/consumer lifetime audit, initialization
and reentry rules, and a fixed performance criterion. The affected differentials
pass against CPython 3.12 on native, LLVM and WASM, covering constructor/new/init
variants, namespace identity/update, slots, vars, pickling, argument
failure/expansion and reentrant release. The runtime semantic gate also passes.
Serial failed-cell retries retain the guards: the initial concurrent LLVM build
reported unequal bytes for identical object inputs, and two WASM builds exceeded
their 600-second guard. Those build incidents do not become semantic passes;
the matching serial receipts establish the affected semantic cells.
The five-pair native hardware A/B uses exact CPython 3.12.16 and the unchanged
per-primitive noise bounds. Plain qualification requires every head slope below
every base slope; material regression on either other primitive is an increase
beyond its existing bound. Allocation/RSS and artifact size remain separate claims.

A single five-pair hardware run on macOS aarch64 compares the same programs and
profiles at base `ca1cdefc68c1` and head `4c8b6bd2e5` (production sources equal to
`540e467c5d`), with exact CPython 3.12.16. Counts below are median N=K minus N=0
`/usr/bin/time -l` retired-instruction slopes; K is 100,000 actual operations.
The paired ratio median need not equal the ratio of the two medians.

| Primitive | Base retired/op | Head retired/op | Paired head/base median | Paired spread | Qualification |
| --- | ---: | ---: | ---: | ---: | --- |
| `plain_construction` | 16670.41749 | 12110.77932 | 0.726705 | 0.169% | separated; within existing spread bound |
| `new_and_update` | 27460.99875 | 27519.57211 | 1.001775 | 29.721% | inconclusive; head/CPython spread exceeds 1.5% bound |
| `dataclass_construction` | 28975.14136 | 28938.18734 | 0.999063 | 0.093% | within existing spread bound; effectively unchanged |

Plain's per-runtime Molt/CPython spreads are 1.180%/1.405%, below its 4.2%
bound, and every head slope is below every base slope. Dataclass's corresponding
spreads are 0.291%/0.291%, below 1.4%. New-and-update retains the head outlier of
35,685.84246 retired/op and its 29.305% head/CPython spread; no sample is discarded
or repeated, and no family-wide no-regression claim follows. Median peak RSS
falls 0.42%, 0.83% and 0.15% for plain, new-and-update and dataclass respectively;
binary size grows 1.58%, 1.50% and 1.41%. These cells establish the scoped plain
construction improvement, not closure of the broader construction gap.

The tables retain the top 15 exclusive functions of **each** build as one union.
Ranks identify membership; `-` means outside that build's top 15. Costs retain
signed startup differencing, including zero for retired functions. Names abbreviate
`molt_runtime::` as `rt::`. Full call edges and review/proof receipts stay in the
private canonical lane artifacts. No sampled percentage is converted to Ir.

#### `plain_construction`

| Rank B/H | Function | Base exclusive/inclusive Ir/op | Head exclusive/inclusive Ir/op | Calls B/H | Mechanism |
| --- | --- | ---: | ---: | ---: | --- |
| 1/- | `<std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&usize>` | 1462.03/2397.04 | 0.03/0.04 | 17.000/0.000 | hashing |
| 2/- | `<core::hash::sip::Hasher<core::hash::sip::Sip13Rounds> as core::hash::Hasher>::write` | 934.93/934.88 | 0.17/0.29 | 16.999/0.004 | hashing |
| 3/1 | `rt::object::dec_ref_ptr_terminal_body` | 439.03/5171.85 | 433.03/4201.62 | 2.000/2.000 | refcount/retirement |
| 4/- | `rt::call::bind::arguments::callargs_builder_is_live` | 380.00/1085.00 | 0.00/0.00 | 5.000/0.000 | builder guards/binding |
| 5/- | `rt::call::bind::arguments::callargs_ptr` | 380.00/1110.00 | 0.00/0.00 | 5.000/0.000 | builder guards/binding |
| 6/2 | `rt::object::seq_access::pin_tuple` | 356.04/356.04 | 356.04/356.04 | 4.000/4.000 | metadata guards |
| 7/3 | `rt::object::object_shape_id` | 344.00/504.00 | 344.00/504.00 | 18.000/18.000 | type/shape guards |
| 8/4 | `molt_dec_ref_obj` | 311.00/3621.02 | 311.00/3614.46 | 7.000/7.000 | refcount/retirement |
| 9/- | `rt::call::bind::arguments::require_callargs_ptr` | 279.00/2019.00 | 0.00/0.00 | 3.000/0.000 | builder guards/binding |
| 10/5 | `rt::object::alloc_object_zeroed_with_aux_policy` | 275.04/708.06 | 287.04/887.01 | 2.000/2.000 | allocation |
| 11/7 | `core::ptr::drop_glue::<rt::object::seq_access::PinnedTuple>` | 268.03/268.03 | 268.03/268.03 | 4.000/4.000 | refcount/retirement |
| 12/8 | `rt::object::heap_lifecycle::take_best_buffer::<u64>` | 264.04/264.04 | 264.04/264.04 | 4.001/4.001 | allocation/retirement |
| 13/9 | `rt::call::bind::inline_cache::try_call_bind_ic_fast` | 263.97/4843.14 | 262.97/4936.90 | 1.000/1.000 | call binding |
| 14/6 | `rt::builtins::frames::namespace::take_invocation_namespace` | 262.00/292.00 | 280.00/310.00 | 1.000/1.000 | call/frame binding |
| 15/10 | `molt_frame_set_col` | 238.00/238.00 | 238.00/238.00 | 7.000/7.000 | call/frame binding |
| -/11 | `<rt::builtins::frames::FrameEntry>::release` | 235.00/268.00 | 235.00/268.00 | 1.000/1.000 | call/frame binding |
| -/12 | `rt::object::builders::canonical_special_singleton_bits` | 224.01/224.01 | 224.01/224.01 | 7.000/7.000 | runtime entry/singletons |
| -/13 | `rt::object::instance_dict_bits_ptr` | 164.00/404.00 | 216.00/396.00 | 4.000/4.000 | dictionary admission |
| -/14 | `molt_callargs_push_pos` | 209.00/2125.04 | 191.00/325.51 | 2.000/2.000 | builder guards/binding |
| -/15 | `rt::object::accessors::guard_layout_match` | 186.00/276.00 | 186.00/276.00 | 2.000/2.000 | type/shape guards |

#### `new_and_update`

| Rank B/H | Function | Base exclusive/inclusive Ir/op | Head exclusive/inclusive Ir/op | Calls B/H | Mechanism |
| --- | --- | ---: | ---: | ---: | --- |
| 1/1 | `rt::builtins::attributes::get_attr_object_ic_with_mode` | 887.99/14060.55 | 887.99/14296.61 | 4.000/4.000 | attribute guards |
| 2/2 | `rt::builtins::attr::cached_class_namespace_mro` | 780.28/1435.81 | 780.26/1435.09 | 10.000/10.000 | attribute guards |
| 3/3 | `<rt::builtins::attr::class_cache::ClassLookupCache>::lookup` | 780.13/780.13 | 780.15/780.15 | 12.003/12.003 | attribute guards |
| 4/4 | `rt::builtins::attr::bind_descriptor` | 757.00/3621.81 | 755.00/3795.60 | 2.000/2.000 | descriptor binding |
| 5/5 | `rt::builtins::type_ops::type_of_bits` | 657.00/1500.00 | 657.00/1500.00 | 12.000/12.000 | type/shape guards |
| 6/6 | `rt::builtins::attributes::class_lookup::attribute_lookup_transaction::<rt::builtins::attributes::attr_lookup_ptr_with_mode::{closure#0}>::{closure#0}` | 642.00/6189.32 | 642.00/6328.69 | 2.000/2.000 | attribute guards |
| 7/7 | `rt::builtins::attr::object_attr_lookup_inner` | 581.00/7085.50 | 581.00/7217.84 | 3.000/3.000 | attribute guards |
| 8/8 | `molt_dec_ref_obj` | 538.99/4507.30 | 538.99/4498.52 | 12.000/12.000 | refcount/retirement |
| 9/9 | `rt::builtins::classes::builtin_classes` | 525.13/524.58 | 525.12/525.10 | 21.005/21.005 | runtime entry/singletons |
| 10/- | `rt::builtins::attributes::class_lookup::type_attr_lookup_ptr_inner` | 452.04/2199.52 | 201.00/2303.48 | 1.000/1.000 | attribute guards |
| 11/10 | `rt::call::bind::inline_cache::load_method` | 430.00/7033.51 | 408.00/7108.19 | 2.000/2.000 | call binding |
| 12/12 | `rt::object::object_shape_id` | 396.00/616.00 | 396.00/616.00 | 22.000/22.000 | type/shape guards |
| 13/13 | `rt::object::builders::canonical_special_singleton_bits` | 384.00/384.00 | 384.00/384.00 | 12.000/12.000 | runtime entry/singletons |
| 14/14 | `rt::builtins::attr::object_attr_lookup_inner::{closure#0}` | 368.00/4694.76 | 361.00/4827.10 | 3.000/3.000 | attribute guards |
| 15/15 | `<std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&usize>` | 344.30/564.49 | 344.30/564.49 | 4.002/4.002 | hashing |
| -/11 | `rt::object::instance_dict_bits_ptr` | 313.12/746.12 | 404.12/732.12 | 8.005/8.005 | dictionary admission |

#### `dataclass_construction`

| Rank B/H | Function | Base exclusive/inclusive Ir/op | Head exclusive/inclusive Ir/op | Calls B/H | Mechanism |
| --- | --- | ---: | ---: | ---: | --- |
| 1/1 | `rt::builtins::attr::bind_descriptor` | 857.00/3562.94 | 866.00/3576.62 | 2.000/2.000 | descriptor binding |
| 2/2 | `<std::hash::random::RandomState as core::hash::BuildHasher>::hash_one::<&usize>` | 766.09/1229.14 | 766.06/1229.10 | 9.000/9.001 | hashing |
| 3/3 | `rt::object::dec_ref_ptr_terminal_body` | 761.98/8914.29 | 755.98/8905.47 | 3.000/3.000 | refcount/retirement |
| 4/4 | `rt::object::alloc_object_zeroed_with_aux_policy` | 554.01/1963.63 | 554.01/1951.39 | 4.000/4.000 | allocation |
| 5/5 | `<rt::object::heap_lifecycle::DetachedEdgeSink>::release_all` | 553.00/2522.95 | 553.00/2522.95 | 3.000/3.000 | allocation/retirement |
| 6/7 | `rt::object::heap_lifecycle::take_best_buffer::<u64>` | 521.01/521.01 | 521.01/521.01 | 8.000/8.000 | allocation/retirement |
| 7/8 | `rt::object::gc::gc_untrack` | 514.71/642.35 | 515.61/643.12 | 4.000/4.000 | GC tracking |
| 8/9 | `rt::builtins::attr::cached_class_namespace_mro` | 468.17/860.92 | 466.39/831.31 | 6.000/6.001 | attribute guards |
| 9/10 | `molt_dec_ref_obj` | 459.00/5344.99 | 459.00/5340.38 | 9.000/9.000 | refcount/retirement |
| 10/11 | `molt_lang_obj_model::release_ptr` | 448.01/1793.49 | 448.01/1796.18 | 4.000/4.000 | refcount/retirement |
| 11/12 | `rt::object::seq_access::pin_tuple` | 445.24/445.24 | 442.80/442.80 | 5.003/4.975 | metadata guards |
| 12/13 | `rt::object::gc::gc_admit_membership` | 437.27/534.31 | 436.48/533.76 | 3.000/3.000 | GC tracking |
| 13/14 | `rt::object::object_shape_id` | 432.00/672.00 | 432.00/672.00 | 24.000/24.000 | type/shape guards |
| 14/6 | `rt::object::instance_dict_bits_ptr` | 410.00/1010.00 | 540.00/990.00 | 10.000/10.000 | dictionary admission |
| 15/15 | `<std::hash::random::DefaultHasher as core::hash::Hasher>::write` | 408.04/408.04 | 408.01/408.01 | 8.001/8.000 | hashing |

### 2.5 Raise and catch (p7, HF-561)

This is the historical constructor-detour measurement. See the
[2026-11-05 refresh](#hf-561-refresh-and-first-implementation-slice-2026-11-05)
for the current mechanism and measured first slice.

210,274 instructions per iteration; CPython 2,038 (103x; wall 15.46 s against 0.09 s). Shares
(12,372 samples):

- **Constructing `ValueError("x")`: 62.6%.** `construct_exception_from_args` finds `__init__` with
  `class_attr_lookup`, which returns a CPython-ABI descriptor. `bind_descriptor` calls
  `molt_cpython_abi::api::descriptor::get`, and the call goes through
  `call_foreign_with_arguments` → `slot_wrappers::method_call` → `raw_call`. The bridge publishes
  a `PyObject` view of the new exception (`publish_cold_pyobj` 20.8%), refreshes and commits its
  exception view (`refresh_exception_view` 20.7%, `commit_exception_view` 9.0%), and keeps handle
  tables in SipHash `HashMap`s behind `parking_lot` mutexes (SipHash 10%+, mutex lock and unlock 3.2% self).
  A probe (`x1`, section 9) shows the same detour at the language level: Molt
  prints `<object object at ...>` for `type(ValueError.__init__)` and for
  `type(ValueError("x").__init__)`; CPython prints `wrapper_descriptor` and `method-wrapper`. This
  is a correctness defect as well as the cost.
- **Raising: 16.7%** (`molt_raise` → `record_exception_with_caller_frame` 16.1%, which builds the
  traceback object eagerly: `exception_materialize_traceback_bits` 6.5%).
- **Freeing the exception: 11.8%** (`molt_dec_ref_obj` → terminal detach, view release).
- Handler match, context set, clear, push, pop: about 3%.

CPython's raise costs one exception allocation, one traceback entry and one frame object per
frame activation (`Python/ceval.c:924-941`, `PyTraceBack_Here(_PyFrame_GetFrameObject(frame))`),
and the handler comes from the code object's exception table (`Python/ceval.c:get_exception_handler`).

## 3. Root causes, stated as missing invariants

1. **No frame invariant.** Nothing states when a frame must exist. The backend emits a frame push,
   homes, argument-zero context and per-operation locations for every function, and the runtime
   implements them as heap-backed thread-local vectors. CPython's invariant: an interpreter frame
   is a fixed-size record on a per-thread stack; a frame object exists only after an observer asks
   for one, and `take_ownership` copies the record out when the object outlives it
   (`Python/frame.c`).
2. **No binding invariant at use sites.** The frontend proves stable module functions
   (`frontend/lowering/sema_state.py:_module_stable_funcs`) and emits `call_guarded`, but the site
   still reads the global by name and asks the runtime for admission. Attribute, method and
   constructor sites carry names as bytes and keep their caches in the runtime. No typed fact says
   "this site reads module slot k" or "this site reads field j of class C".
3. **No context invariant.** Compiled code never owns the execution context. Each helper rebuilds
   it, and per-thread state that is a single word (eval breaker, recursion depth, pending
   exception, current location) is reached through calls. The thread record from frame-cost
   stage 1 (`THREAD_RECORD_EXCEPTION_PENDING_OFFSET`, `THREAD_RECORD_PENDING_INVOCATION_OFFSET` in
   `molt-codegen-abi/src/lib.rs:165-168`) is the correct seed: it has two fields.

Two contributing causes follow from these: the effect oracle treats frame-home stores as may-throw,
which keeps `check_exception` after each one (HF-185), and immediate values carry no type fact that
elides reference counts.

## 4. Target architecture

### 4.1 Execution context

Every compiled function takes `ctx: *mut ThreadRecord` as its first native argument, as CPython
passes `tstate` (`Include/internal/pycore_pystate.h:_PyThreadState_GET` reads `_Py_tss_tstate`
once; internal functions take `tstate`) and Wasmtime passes `VMContext`. Entry trampolines (the
`*_molt_trampoline_N` symbols and the C-API entry points) read TLS once and pass `ctx`. Runtime
slow paths take `ctx` and never read TLS.

`ThreadRecord` layout is generated in `molt-codegen-abi` (one authority, consumed by every
backend):

| field | type | written by | read by |
|---|---|---|---|
| `pending_exception` | u8 (exists) | runtime raise paths | inline checks |
| `eval_breaker` | u32, atomic relaxed | signal handler, GC scheduler, async queue, stop-the-world requests | polls |
| `py_recursion_remaining` | i32 | inline call prologue | inline check, `sys.setrecursionlimit` |
| `frame_top` | `*mut FrameRecord` | inline push/pop | tracebacks, `sys._getframe`, frame objects |
| `pending_invocation` | u64 (exists) | direct-call handoff | callee entry (removed in stage 3) |
| `runtime_generation` | u64 | runtime init/shutdown | cache validity (HF-200) |
| `owner_thread_id` | u64 | thread attach | biased refcount (free threading) |

**Invariant J1.** On the non-raising path, compiled code reaches per-thread execution state only
by loads and stores at generated offsets from `ctx`. Check: a codegen census test lists the runtime
symbols each backend may import on a fast path; `molt_thread_record`,
`molt_async_work_poll_and_exception_pending`, `molt_recursion_enter_fast`/`exit_fast`,
`molt_frame_set_col`, `molt_frame_set_line_col`, `molt_trace_set_line`, `molt_trace_enter_slot`,
`molt_trace_exit`, `molt_frame_homes`, `molt_frame_context_set` and
`molt_function_direct_call_eligible` are not in any list. The type form: runtime fast-path helpers
take `&ExecCtx<'t>`, a token that only the attach transition creates, so a helper cannot be called
without a context in scope.

### 4.2 Calling convention

Each compiled function has two entries (mypyc's `CPyDef_f` native function and `CPyPy_f` wrapper;
Cython's `cdef`/`cpdef`; CPython's PEP 590 vectorcall, `Objects/call.c:_PyFunction_Vectorcall`):

- **Native entry** `f$n(ctx, a0..an) -> Value` with exact positional arity, closure environment as
  an explicit parameter, and a declared result carrier. Its signature is a typed fact,
  `NativeSignature { arity, closure, params: [Carrier], result: Carrier, may_raise, frame: FramePlan }`,
  produced once per function by the frontend/TIR and consumed by every backend.
- **Generic entry** `f$g(ctx, callable, args*, nargs, kwnames) -> Value` binds defaults, keywords,
  `*args` and `**kwargs` per the call-binding contract, then calls `f$n`. Python callers, the
  runtime, and unproven sites use it.

Each call site carries a typed binding class, decided by the frontend from binding-flow facts:

| class | when | lowering | cost |
|---|---|---|---|
| `Proven(FunctionId)` | the verified namespace premise proves the slot holds this function at the site (`docs/design/verified_namespace_premises.md`: only the owning module writes it, verified over the closure) | `call f$n(ctx, args)` | call + moves |
| `Guarded(FunctionId, GlobalSlot)` | stable by the frontend's premise, not proven over the whole closure | load slot; compare with the function object's address; equal: `call f$n`; else `call f$g` | 1 load, 1 compare, 1 branch |
| `Monomorphic(SiteId)` | unknown callee | inline compare of the callee with the site record's cached function; hit: `call` cached native entry; miss: `f$g` path that refills the site record | 2 loads, 1 compare |
| `Dynamic` | anything else | generic call | runtime |

The callee prologue does the recursion check as CPython does (`Python/ceval.c:_Py_EnterRecursivePy`:
`py_recursion_remaining-- <= 0`): one decrement and one branch in `ctx`, cold path
`molt_raise_recursion_error(ctx)`. A native stack-pointer check against a limit in `ctx` guards the
host stack on native (CPython 3.14 stack protection; HotSpot guard zones); WASM uses Wasmtime's
`max_wasm_stack` trap plus the depth count.

**Invariant J2 (call refinement).** For every site, when its guard holds, the native call and the
generic call produce the same observable result, exceptions and reference effects. Check: a test
build mode forces every guard to miss; the differential corpus must produce identical output in
both modes (the "reference meaning" rule of HF-485). `Proven` sites carry the premise's proof
identity; the premise checker refuses the build when a write to the slot appears.

### 4.3 Frame records and lazy frame objects

Settled 2026-11-10 and landed in two steps: records live in the per-thread frame arena, not in
the native stack frame (stable addresses, the same on every backend). The record chain headed by
`ThreadRecord.frame_top` is the runtime's one frame authority
(`molt-runtime/src/builtins/frames/records.rs`). A record is six header words (`prev`, `function`,
`code`, `flags`, `observer`, `location`; `molt-codegen-abi` `FRAME_RECORD_*`) followed by the
frame's homes; its base is the homes base compiled code already addressed.

- A **synchronous function** pushes and pops its record inline. One generator per transition
  (`molt-codegen-abi/src/frame_record.rs`: `generate_frame_push`, `generate_frame_pop`) serves
  Cranelift (inline), LLVM (an always-inlined body per frame shape) and WASM (one push body and
  one pop body per module). The push takes the handed function from
  `ThreadRecord.pending_invocation` when its code owns this slot and the arena chunk has room
  (`arena_top`, `arena_limit`); the record retains its function and code, as CPython 3.12 holds
  `f_funcobj` and `f_code` strong, and reads globals and builtins from the function. The pop
  unlinks, releases the homes in the target's clear order (the `trace_enter_slot` value carries
  the home count and that order), releases function and code, and returns the arena top.
- **Every other frame** (a module or class body, a stateful activation, a runtime-implemented
  Python body, an entry with no handed function or no room in the chunk) is linked by the runtime
  (`molt_frame_record_enter`) as an owned record whose namespace block sits below its header.
- A pop enters the runtime (`molt_frame_record_exit`) only when the record's flags word is not
  zero: an observer attached a frame payload, the runtime owns the record, or it opened an arena
  chunk.
- Zero-argument `super()` reads a synchronous frame as CPython's `super_init_without_args` does:
  "no arguments" is `co_argcount == 0`, argument zero is home 0 and the class cell is the home of
  the code's `__class__` free variable. Such a frame publishes no context
  (`molt_frame_context_set` remains for stateful and class-body frames).

`molt_trace_enter_slot`, `molt_trace_exit`, `molt_frame_homes`, `FRAME_STACK` and
`TRACE_FRAME_PUSH_STACK` are deleted. The original sketch follows the slice-2 notes.

**Slice 2 (not started; state at 077e117176).** A stateful activation (generator, coroutine,
async poll) still enters through `molt_frame_record_enter`: the runtime links an owned record
whose namespace block holds the task as `activation_bits`, restores the task's saved location
word, and `molt_frame_record_exit` saves it back (`FrameEntry::exit`). Its entry carries no
frame shape (no `trace_enter_slot` value), and its frame still publishes `FRAME_CONTEXT_SET` on
entry and after each resume (`function_lifecycle.py`, `async_gen.py`). Slice 2 keeps the record
in the task payload, as CPython keeps `gi_iframe` in the generator: resume links it to
`frame_top` and suspend unlinks it, both through new `frame_record` generators, so no backend
hand-writes them. It then reads `super()` from the payload's slots, as synchronous frames read
their homes, and removes `FRAME_CONTEXT_SET` from stateful frames (class bodies keep it). The
flags word uses bits 0-2 (`FRAME_RECORD_OBSERVED`, `OWNED`, `CHUNK_FIRST`); a `STATEFUL` bit is
free. Slice 2 must carry the differential cells `frame_record_observed_frames` (a suspended
`gi_frame`) and `sys_exception_across_resume`. Its measurement adds a generator loop and an async
call loop to the programs of section 9.
Two step-2 measurements are also still open: WASM instruction rows (WASM runs the differential
cells but has no instruction row) and atomic-refcount builds, where the record's two retains and
each home release are runtime calls.

A function whose frame can be observed keeps a `FrameRecord` in its own native stack frame (on
WASM, in the shadow stack):

```
FrameRecord { prev: *FrameRecord, code: CodeId, function: *Function, location: u32,
              flags: u32, homes: [Value; plan.home_count] }
```

- **Push** (prologue): `rec.prev = ctx.frame_top; rec.code = K; rec.function = callee;
  ctx.frame_top = &rec` — four stores, no reference counts. The record borrows the function and
  its code: the call site keeps the callable alive until the call returns (it releases the
  callable after the return today). Globals and builtins come from the function, as
  `_PyInterpreterFrame` borrows `f_globals` and `f_builtins`. Materialization takes strong
  references.
- **Pop** (epilogue and every exit edge): `ctx.frame_top = rec.prev` — one store, plus the
  copy-out below when `flags & MATERIALIZED`.
- **Location**: before an operation that may raise or may observe frames, one store of a 32-bit
  location index into `rec.location`. The index selects (line, col, end_col) from a static
  per-code location table, decoded only when a traceback or `f_lineno` needs it (CPython
  `Objects/locations.md`; HF-261). Native may later derive the index from the return address and
  drop the store; WASM keeps the store.
- **Homes** are the record's own slots. In stage 1 every local store still writes its home, as
  today, but as a plain store that cannot raise and needs no check. Stage 6 writes homes only at
  observation points.
- **Argument-zero context** (`super()`, `__class__`) is written only by functions whose body uses
  zero-argument `super` or `__class__`; the frontend already knows this (`class_cell` in
  `frontend/lowering/function_lifecycle.py`).
- **Frame objects** materialize on observation: traceback construction, `sys._getframe`,
  `f_back`, `inspect.currentframe`, `locals()` and C-API `PyEval_GetFrame`. Materializing sets
  `MATERIALIZED` and links the object to the record. On pop, a materialized record copies its
  homes and location into the frame object, which then owns them (`Python/frame.c:take_ownership`).
- **Generators and coroutines** keep their record in the task payload, as CPython keeps
  `gi_iframe` in the generator; resume links it to `ctx.frame_top`, suspend unlinks it.
- **Module globals for name reads** come from `rec.function`'s globals (CPython reads
  `frame->f_globals`), which replaces `frame_stack_active_globals_bits()`.

Which functions need no record at all: a function that cannot raise, cannot call unproven code and
cannot suspend (a leaf over proven scalar operations) has no observer. The frontend computes
`FramePlan::None | Record { home_count, uses_class_cell }` from TIR effect facts.

**Invariant I1 (observational equivalence).** Every frame observation returns what the eager model
returns: the chain of `f_code`, `f_lineno`, `f_back`, `f_globals` and the traceback lines, and
`f_locals` at the observation point. Typed state machine for a record:
`Unobserved → Materialized → Detached` (detached means the native frame returned and the frame
object owns the copied state); no transition skips a state, and only `Materialized` may detach.
Check: the existing traceback, `sys._getframe`, warnings `stacklevel`, logging `findCaller`,
generator and `super()` differential tests on native, LLVM and WASM, plus a new property test that
raises at random depths and compares the full traceback (file, line, column carets) with CPython.

#### Executing source locations (HF-764)

The location migration can precede the full native-stack record migration. Reuse the existing
stable-address, chunked frame-home arena and lazy binding owners; do not introduce another frame
registry or a location cache. This design is reviewed but does not establish implementation or
backend qualification.

The frontend must preserve each executable expression's start/end line and columns, including
multiline expressions. At serialization, one deterministic location table belongs to each Python
code slot, including its inherited module chunks. Emit the table at code creation, using the
documented [CPython location encoding](https://github.com/python/cpython/blob/v3.12.0/Objects/locations.md).
The emitted program never reparses source files to recover a location. A typed `LocationId`
selects a row; `f_lasti` is twice that index, so `co_positions()[f_lasti // 2]` describes the
executing site. It does not emulate CPython's version-specific bytecode offsets. Differential
oracles compare the selected source coordinates rather than the representation-specific integer.
An arbitrary positive offset is insufficient evidence.

An explicit `frame_set_location(LocationId)` publishes before operations that may raise or observe
Python state, and before every source return, yield, suspension and exceptional exit. The
frontend emits it conservatively; the cost addendum below ("Location publication cost") removes
every publication that no reader can observe, and an Origin edge materializes its own site.
Only an existing proven pure, non-throwing, non-callback effect fact permits omission; missing
facts are conservative. The opcode has no SSA operands/results, a nonnegative immediate, no Python callback
or exception, and an impure side effect. Use the current conservative memory-effect representation
if it cannot distinguish frame-observation memory. Optimizers may coalesce adjacent redundant
publications only when every incoming path already carries the required location; they cannot
move a publication across an observation, failure or terminator. An exceptional check preserves
the known location on its normal fallthrough, so an identical fallthrough store is redundant.
Its exceptional target resets location knowledge; the first observing operation publishes
its source site. Other control-flow boundaries reset that knowledge conservatively.
Backend-generated exits retain their originating source terminator's location.

Region labels, jumps, try markers and non-callback exception-state transport do not
publish a new source site. Their structural adjacency is part of the region model.
The frontend derives non-observing controls from the generated control, callback
and automatic-check facts; throwing guards remain observers. Compiler-owned home
restoration forwards the incoming location to any release callback instead of
publishing synthetic scope coordinates. Ordinary binding writes retain their
publication obligation. Await resumption retains and clears its hidden result
carrier before taking an error edge. A fresh post-resume owner keeps the awaited
task alive while its hidden slot is cleared. The receiving Origin transfers
directly to the lexical handler; the owner's normal release or canonical
exception-edge release follows capture, before authored cleanup proceeds.

Reserve a raw location word beside each executing frame's binding homes, even for module bodies
and frames with no local slots. The existing home-base lend supplies its address once per
invocation, only after successful entry; inherited chunks borrow that same frame's storage.
Native, LLVM and WASM lower publication to a plain store at one canonical ABI offset. The borrowed
pointer escapes to the runtime owner: LLVM cannot treat it as private/noalias, and runtime calls
that observe it cannot be marked `readnone` or `inaccessiblememonly`. Retire the replaced line and
column update calls together with their consumers. Optional line-debug diagnostics do not own
frame locations. Measure both the stores and deleted calls before claiming a cost improvement.

Frame objects read the current location through one shared lazy activation owner, as their locals
views already share binding storage. Materializing a view must not copy a permanently stale
`f_lineno` into its dictionary. Before stack storage retires, freeze the last location in the
shared owner and detach its borrowed address, before releases can reenter Python. Retained frame
views then remain valid after return, recursion and arena reuse. Unobserved ordinary execution
adds no per-call heap allocation. Include every owned edge in GC traversal, destruction and
runtime teardown; never retain a pointer into a movable frame-stack vector.

Stateful activations preserve their location in the existing task auxiliary frame context:

- **Created:** the task owns its explicit entry location and optional lazy view. CPython 3.12's
  unstarted generator reports offset zero and the defining line; it is not an ordinary frame with
  an absent location (`-1`).
- **Running:** successful entry copies the saved location to arena storage and reattaches any
  existing view. The active entry keeps the task alive, and observations share the same owner.
- **Suspended:** save the latest location in the task context and redirect the view to that saved
  word before unlinking and returning arena storage. Save it even when no view exists.
- **Completed:** freeze retained views before task storage retires; new `gi_frame`/`cr_frame`
  observations return `None`. Close, throw, failed resume, GC and destruction follow the same owner.

Traceback membership is an independent obligation of exception propagation. Each selected entry
must retain its particular activation's shared frame owner and freeze its location before
`finally` or cleanup changes the live site. Code identity cannot identify an activation under
recursion. `tb_lineno`/`tb_lasti` decode the frozen site while `tb_frame` can continue moving.
Do not reconstruct offsets by matching only a line or selecting its widest span.

The native location prototype exposed a membership defect: propagation through `finally` retains
the raising frame but omits the module's receiving call site. The settled correction follows
[CPython 3.12's event model](https://github.com/python/cpython/blob/v3.12.0/Python/ceval.c): `error`
calls `PyTraceBack_Here`, while `exception_unwind` skips it. Every exceptional check carries a
static, typed kind generated from the opcode authority:

- **Origin:** an operation fails, a call/await/yield-from receives an error, explicit raise,
  bare raise without an active exception, or throw/close injection. Prepend exactly this activation
  at its frozen location to the exception's current traceback, on the taken error edge only.
- **Unwind:** valid bare raise, finally/with/except cleanup reraising, unmatched handler dispatch,
  synthetic follow-on checks, exceptional loop breaks, finally pending polling and pending-error
  function exit. Forward the exception without adding an entry.

No deduplication or runtime propagation-episode state exists: repeated Origin events through one
activation produce repeated entries. Capturing the current traceback honors editing and
`with_traceback`; valid bare raise preserves those edits while the receiving caller's Origin adds
its call site. A runtime helper suppressing a callback error takes no caller error edge.
Delete eager nearest-handler range capture; it cannot remain a fallback. Split any check that
joins pending sources of different kinds, and reject unclassified/mixed checks in the verifier.
All five backend consumers match both kinds. Origin capture is cold; the normal path adds no call.
Compiler transport carries a typed producer-source Unwind fact through serialization and TIR,
preserving pending source provenance across frame-context restoration and module-failure
bookkeeping. Conservative runtime failure effects do not by themselves manufacture Python
Origin events. Authored guarded bodies keep their own handler, source loops check their failing
operations, and await captures its received Origin before forwarding cleanup. The verifier uses
these producer facts at joins; runtime custody and effect analysis retain their conservative rules.

Two independent, serial read-only design reviews required explicit terminal publication, stateful
view reattachment, exception capture timing and membership, and optimizer memory effects. The
location contracts above incorporate those findings; the parent's event model settles the
second-round propagation findings. Required independent adversaries cover retained views
ending with a pure return, multiline and same-line expressions, callbacks inside optimized loops,
recursive activations, internally caught and propagating exceptions, bare/explicit reraising,
cleanup that raises a new exception, and retained views during resumed callbacks after arena reuse.
Include repeated traversal of one caller by the same exception, and bare reraising after clearing
or editing its traceback; compare chain multiplicity as well as order and coordinates.
Compare exact positions and traceback membership/identity with CPython, on native, LLVM and WASM.

Qualification also requires the owning loop/per-op and recursion/per-op instruction A/B, with
identical source/toolchain/profile/workload identities and the primitive authority's minimum five
paired samples and axis noise bound. An increase beyond that bound blocks acceptance; an increase
inside it is inconclusive, not a win. Repeat only an affected unresolved cell. Do not change the
performance ratchet or methodology to admit a candidate. Record runtime calls, whole-program time,
memory and binary size alongside counters. Generated synchronization, touched-reader checks and
the landing precheck remain separate required evidence.

#### Separate normal completion from exception dispatch (HF-764, 2026-11-06)

General `try` and `try*` lowering with handlers must preserve separate normal and
exceptional entries. Successful completion closes its region and jumps directly to the
continuation containing `else` and normal-finally work. The exceptional landing closes
its region and enters handler dispatch directly. Only that landing reads the raised slot
for dispatch; successful completion executes no pending-slot observer, comparison,
negation or branch for dispatch. Existing normal-finally observers remain unchanged.
Preserve the existing handler bodies, async exception-slot
stores, finally context and cleanup ownership. Handler-less `try/finally` retains its
marked pending observer. The specialized synchronous split remains the normal form for
its existing subset.

This establishes the existing invariant at the producer: valid general try lowering
verifies, every executable Origin is captured exactly once, and a real mixed-source check
is rejected. The shared verifier and its physical custody traversal need no predicate
projection. Previously the general lowering joined normal and exceptional completion,
then branched on a pending-slot None test. The verifier conservatively explored both arms,
allowing an exceptional Unwind into the normal `else` arm and deriving Mixed at its first
potential failure. The producer split removes that infeasible path from the CFG itself.
It also makes normal `try*` completion enter `else` and normal-finally work before cleanup.

All backends consume the same producer split. Qualification must cover synchronous,
async, module and exception-group forms, failure in `else`, matched and unmatched handlers,
nonlocal exits, and finally precedence against CPython. Existing exception and traceback
oracles, including the three frame differentials, own observable membership, multiplicity
and frozen coordinates. Keep physical region/MatchRef ownership and the async captured
exception slot intact across suspension. Measure a try-heavy program's instructions,
compile time and artifact size before and after; removal of dispatch work is not itself
a measured performance result.

Equal normal and exceptional destinations do not make a check transparent: an Origin
captures a traceback entry. Frontend check/jump threading may remove only an Unwind check
with no payload or asynchronous-work marker and no lexical-handler or landing destination.
Plain jump threading remains eligible. A failing operation in `else` must still create its
own Origin; matching destinations are not an oracle for event redundancy.

The verifier-side SSA predicate proposal is rejected: it would add freshness/presence
machinery, require preservation across optimization and drop pipelines, and weaken the
existing producer-split contract. Marked-finally-observer transition gaps and detection of
an Origin dropped before return are separate findings; this correction does not change
their verifier rules or runtime semantics.

#### Location publication cost (HF-764 correction, 2026-11-04)

The owner's acceptance: the loop primitive costs no more instructions per iteration than base,
recursion keeps its gain, and every semantic proof stays green.

**Measured anatomy.** Loop primitive, N=1,000,000, `otool -tV` of `_loop__run`. Retired
instructions rose from ~367.9 to ~401.7 per iteration (+33.8). The executing loop is the
guarded counted range loop (`molt_guard_builtin_bindings` admits it), not the generic
`molt_iter_next_unboxed` loop beside it. Its hot path is 128 instructions in base (22 loads,
12 stores, 9 calls) and 163 in head (41 loads, 20 stores, 9 calls): +35, which matches the
counter. The generic loop also grew (104 to 128) but does not execute. Head adds 36 instructions
and drops 1 register move:

1. Six publications per iteration (`frame_set_location` 6, 6, 7, 9, 8, 6): 16 instructions. They
   precede the header `lt`, the store of `i`, two binding reads, `inplace_add` and the latch
   `add`. None of these can run Python code: the operands are exact ints, and the stores
   release only ints.
2. Six reloads of the frame-home base from a spill slot. Cranelift hoists the loop-invariant
   location constants 7, 8 and 9 into callee-saved registers; the home base loses its register.
3. Three pending-exception reads that base does not execute, 4 instructions each, and one spill
   of the `lt` result for the first: 13 instructions. See M3.
4. One reload of the loop bound from a spill slot (register pressure).

**M1. An Origin edge materializes its own site.** An Origin check observes the failure of one
operation, so its location is a constant when one publication reaches it on every path. The
placement pass (M2) computes it with a forward reaching-publication analysis over the
conservative stream and records it on the check as the TIR attr `frame_location`; SimpleIR
carries it as `OpIR.frame_location`. The schema rejects it on any operation that is not an
Origin check. On the taken Origin edge, native, LLVM and WASM store the constant into the
frame's location word, then call `molt_exception_frame_origin`. On a check marked
`async_work_poll`, they also store it at the start of the cold service path, because a pending
call or signal handler is Python code that can read the frame. A check that no single
publication reaches gets no attr and stays a reader. The normal path has no store. WASM needs
no return address: the store is explicit.

**M2. One placement pass deletes unobservable publications.** `frame_location_placement` is the
last pass of the terminal drop pipeline, so it sees final types and inserted releases. It is a
backward liveness of the location word over the CFG, exception edges included.

- *Writes:* `frame_set_location`; the taken edge of an Origin check, and the service path of a
  poll, that carry `frame_location`; function entry. The runtime writes ABSENT at a fresh frame
  entry (`FrameBindings::enter`) and restores the saved word at a resume, so the word never holds
  another code object's LocationId.
- *Reads* (fail-closed: an operation is a read unless the facts below clear it): (a) an
  operation whose typed effects (`op_effects_with_types` over the exact scalar map) may call
  Python, except the release callbacks in M2.1. Native frame introspection (`getframe`,
  `frame_locals`), warnings and attribute access are calls or carriers outside
  `callback_free_copy_kinds`, so they are reads; (b) a state-machine opcode
  (`opcode_is_state_machine_table`: suspension saves the word, resume restores it); (c) a
  `Return` terminator. Every frame-retiring path ends in one and first runs `trace_exit`, also a
  read: normal return, the pending-error exit, generator completion and backend-generated exits
  alike. So a `finally` body's last publication before an exceptional exit stays live; (d) an
  Origin check or a poll without `frame_location`.
- An Unwind edge forwards the handler's liveness unchanged. A `TryStart` edge needs no term:
  only `Raise` transfers through it (WASM native EH), and `Raise` is a read (an operation, not a
  check; its transport check is Unwind).

The pass deletes each publication that no read can observe. Liveness preserves the word's value
at every read, so every remaining reader sees what the conservative stream showed it. The pass
only adds attrs and deletes publications, so it is idempotent. Inlining cannot mix two code
tables in one word: the TIR inliner refuses any callee with an execution context
(`inliner/eligibility.rs`). A typed target lowering feature, `cold_origin_frame_location` in
`SimpleIrLoweringFeatures`, gates the pass. It commits a backend to both cold stores and is true
for native, LLVM and WASM. Luau and Rust keep every publication: Luau publishes line and
columns with each location and has no code table to recover them from a LocationId, and the
Rust backend captures no Origin.

**M2.1. Which release callbacks read the location.** A release can start a finalizer
(`__del__`, a weakref callback) that reads the frame. Two releases are not reads:

1. `DecRef` of a temporary. Drop insertion places it at the value's last use, which is not a
   CPython release point, so the statement in which such a finalizer runs is not a CPython
   contract today. Its finalizer sees the frame's last surviving publication.
2. A frame-home store or clear (`frame_home_store`, `frame_home_clear`) whose previous binding is
   proven finalizer-free: unbound or an exact builtin scalar. A forward slot-content analysis in
   the pass proves it. The activation's own entry (`trace_enter_slot`) starts a fresh
   synchronous frame with every home unbound (the arena zeroes them); a stateful or inherited
   frame starts unknown. A store records whether its value is an exact scalar; a clear or take
   leaves the home unbound; `frame_locals_set` makes every home unknown. Only those operations
   write this activation's homes.

   The second boundary: a callback that rebinds a local through a PEP 667 proxy does not clear
   these facts. A finalizer of a value injected that way, released by a later compiled rebinding,
   sees the last surviving publication. Clearing the facts at every callback is not affordable:
   `range(n)` converts its bound with `operator.index`, which may call Python, so every counted
   loop would keep a store before its first rebinding of the loop variable, in every iteration.

Every other release keeps its read: rebinding or `del` of a value not proven finalizer-free
(`ResourceWarning` lines depend on it), `DeleteVar` and `DelBoundary`. The classification is
typed: `op_kinds.toml` lists `temporary_release_opcodes` (`DecRef`) and
`binding_release_copy_kinds` (`frame_home_store`, `frame_home_clear`, whose only Python
callback is releasing the previous binding of home `value`), rendered as exhaustive
predicates; the generator rejects a member whose callback capability is false. Out-of-band
observers, such as `sys._current_frames` or faulthandler from another thread, see the last
surviving publication. That is a second explicit boundary; CPython's own answer there depends on
where the eval loop yields.

**M3. Restore base's check set in the counted range loop.** The lane removed the range loop's
check suppression, so the frontend now emits an Origin check after the header `lt`, after the
loop-variable store, and after the latch `add`; TIR keeps three of them, because Copy carriers
(`frame_home_store`, `line`) are conservatively may-raise and the `stop` bound (an
`operator_index` result) has no exact type. That index machinery cannot raise: `range()` bounds
come from `_emit_range_bound` (an exact int, or `operator.index`, whose result has exact type
`int` since CPython 3.10); the index starts at an exact int and adds an exact-int step, and an
overflow promotes to a big int instead of raising; a simple-name binding stores memory, and a
finalizer error during its release is unraisable. Resource exhaustion is outside the semantic
exception lattice (`op_kinds.toml`). So the frontend restores base's suppression scopes around
the header (`loop_start`, `loop_index_start`, `lt`, `loop_break_if_false`, simple-name binding)
and the latch (`add`, `loop_index_next`, `loop_continue`), byte for byte. The scope marks those
operations `exception_source = unwind`, which the event verifier accepts: they produce no Origin
event. Non-simple targets stay outside the scope, as in base. A frontend test pins the absence
of checks in that machinery.

**M4. Two typed facts the placement pass needs in that loop** (found when M2 first ran: it kept
two publications per iteration).

1. The header `lt` compares the index with the `operator_index` result. TIR had no exact type for
   that result, so `lt` could call Python and stayed a read. `operator.index` returns an exact
   `int` (CPython 3.10+, `PyNumber_Index`). `op_kinds.toml` now lists
   `exact_int_result_copy_kinds = ["operator_index"]`; the exact scalar solver gives the result
   `BigInt`, an exact int of any magnitude with a boxed carrier, as it does for `ConstBigInt`.
   The representation plan floors integer names to `MaybeBigInt`, so no consumer unboxes it
   without a range proof.
2. `binding_alias` only retains its operand (+1). It joins `callback_free_copy_kinds`.

**Invariants.**

1. At every read, the word holds the value the conservative stream held there (M2 liveness),
   except a finalizer of a release that M2.1 exempts or of a value injected through a PEP 667
   proxy, which sees the last surviving publication.
2. An Origin capture records the failing site's own LocationId on native, LLVM and WASM when one
   publication reaches its check, and otherwise reads the preserved word (M1).
3. Measured gate, per backend: the normal path of a loop whose operations cannot call Python,
   except through M2.1 releases, contains no location store. The absence of home-base reloads
   is a register-allocation outcome; the `otool` count gates it on native. On LLVM and WASM the
   cold constant is an immediate at its use.
4. A target without `cold_origin_frame_location` runs no placement pass.

**Consumers.** SimpleIR schema and transport (`ir.rs`, `ir_schema.rs`, SSA lift,
`lower_to_simple`, `addressed_program.rs`); `op_kinds.toml` and the generators (three new lists, one `callback_free_copy_kinds` entry);
`target_info.rs` (feature); the exact scalar solver (`exact_int_result_copy_kinds`); the pass
and its pipeline slot; native `exception_control.rs`,
`scalar_carriers.rs` and the frame-home lend predicate; LLVM `op_dispatch.rs`,
`value_materialization.rs` and the lend predicate; WASM `emission.rs`, `pending_work.rs`, both
check emitters and the lend predicate; frontend `loop_lowering.py` (M3). A function whose last
publication the pass deletes still lends its homes when a check carries `frame_location`.

**Tests.** Pass unit tests: a counted loop of exact-int operations keeps no publication; a call
keeps its publication; a release-only store of a finalizer-free binding does not, and one after
a call does; `Return`, a state-machine opcode, an Origin check reached by two different
publications, and an Unwind edge into an observing handler each keep the reaching publication;
an Origin check reached by one publication gets that `frame_location`. A frontend test: the
counted range loop's header and latch emit no exceptional check. A differential case with
`range()` bounds beyond 64 bits guards M4.1. The nine core cells (frame
fields, lifecycle, event adversaries on native, LLVM and WASM), the whole
`tests/test_python_execution_frame.py` and the frontend location witnesses stay green. Cost:
loop and recursion instructions-retired A/B against `origin/main`, N=1,000,000, five pairs, and
the static hot-path count from `otool`.

**Prior art.** HotSpot and V8 record a bytecode index at each call site and recover it at
observation (PcDesc, safepoint tables); Go recovers lines from return PCs (`pclntab`). They pay
nothing on the hot path but need a readable return address. WASM cannot read one, so Molt keeps
explicit stores and moves them to the reads, as LLVM's dead-store elimination does for any
memory location whose loads it can see.

**Review record.** One fresh-context Goldfish design review (egm-design, 2026-11-04) raised two
blockers and six majors. Adopted: every frame-retiring path is a read (c); entry is a write and
the runtime initializes the word; fail-closed reads; the inliner fact; `frame_location` computed
in the pass, not copied from the frontend; M2.1 narrowed from all releases to temporaries and
proven finalizer-free bindings, with the PEP 667 boundary stated; M3 made concrete; Invariant 3
made a measured gate. Not adopted:
passing the LocationId to `molt_exception_frame_origin` as an argument. It is the same constant
in a register, so it neither removes the hoisting risk nor the poll-path store, and it changes
the runtime ABI of five backends.

### 4.4 Exceptions

Molt keeps the error-return protocol: a failing operation sets `ctx.pending_exception` and returns
a sentinel; the caller tests it. This is CPython's C protocol, Cython's and mypyc's. The protocol
costs one load and one branch per may-raise operation and nothing per call beyond that.

**Refuse Itanium-style landing pads for Python exceptions [I].** Table-driven unwinding (Codon uses
LLVM's) makes the non-raising path one branch cheaper but makes each raise cost a two-phase native
unwind. Python raises often (`StopIteration`, `KeyError`, `AttributeError` under `hasattr`,
`getattr` defaults), and CPython's raise-and-catch is 2,038 instructions. Keep CPython's trade:
zero-cost `try` (no instruction for entering a `try` block, exception tables in CPython
`Objects/exception_handling_notes.txt`), cheap raise. In compiled code a `try` already costs
nothing until a check fails; the handler is the branch target.

Changes:

- **Checks only where an operation may raise.** TIR purity facts (`op_kinds.toml`
  `primitive_operator_effect_cases`) decide; frame-home stores and location stores are `nothrow`.
- **Lazy tracebacks.** On the error edge out of a frame, compiled code appends `(rec.code,
  rec.location)` to the in-flight exception's traceback buffer (mypyc `CPy_AddTraceback`; CPython
  `PyTraceBack_Here` at `Python/ceval.c:941`). Python traceback objects and frame objects
  materialize when code reads `__traceback__`, `tb_frame` or `f_locals`, or when the exception
  reaches the top level.
- **Native exception classes.** Builtin exception classes are native Molt classes with native
  `__new__`/`__init__` slots; construction from compiled code is
  `alloc_exception(ctx, CLASS_HANDLE, args)` with generated class handles (HF-207). Exceptions never
  pass through the CPython-ABI bridge unless C code observes them.
- **Handler match** compares the pending exception's class pointer with the handler's class handle,
  with a subclass walk on miss.

**Invariant I2.** The non-raising path of a `try` body executes no instruction that a body without
`try` would not execute. Check: anatomy of a function with and without `try` around the same body
must be equal in hot-path instructions (gate in the anatomy tool).

### 4.5 Compile-time binding and inline caches

- **Module globals.** Each module has a slot array; its `__dict__` maps names to slots (CPython's
  dict keys and `LOAD_GLOBAL_MODULE` keys-version index; mypyc's static module attributes). A
  global read at a site is `load slot[k]` plus an unbound check. Writes through `globals()`,
  `setattr(module, ...)` or `module.__dict__[...]` go through the slot. Builtins read through a
  slot whose cell the module's builtins scope owns, guarded by a builtins version.
- **Attributes.** A site with a known class (`ClassId` from the binding flow) and a fixed layout
  lowers to: load the object's class pointer; compare with the expected class; load or store field
  `j` at a generated offset. A miss calls the runtime, which updates the site record in a data
  section (V8 hidden classes; HotSpot field offsets; CPython `LOAD_ATTR_INSTANCE_VALUE`). Stores
  apply the class's attribute protocol fact (ee15359a6): plain classes take the inline store, data
  descriptors take the slow path.
- **Methods.** `obj.m(args)` with a class-version guard lowers to a native call of the resolved
  function with `self` as argument zero: no bound-method object, no name hashing (CPython
  `LOAD_ATTR_METHOD_WITH_VALUES` + `CALL_PY_EXACT_ARGS`).
- **Constructors.** `C(args)` with a proven or guarded class whose `__new__` is `object.__new__`
  lowers to inline allocation of the class layout and a native call of `C.__init__$n(ctx, self,
  args)` (CPython 3.13 `CALL_ALLOC_AND_ENTER_INIT`).
- **Module attributes.** `struct.pack` with `struct` a proven module binding is a global slot read
  of that module: no class MRO walk.
- **Names.** Attribute and method names are interned name handles in the op format (typed keys,
  HF-202), never `(ptr, len)` bytes.

**Implementation (lane module-slots, 2026-11-11).** A module's layout is the sorted
non-dunder names its own code reads, writes or deletes as globals; the module registry
(schema 6) carries it. The namespace dictionary owns every value; its slot table mirrors
them inside the dictionary's commit primitives (insert, replace, remove, compaction,
reset), with a row-to-slot inverse so a replacement needs no probe. A non-exact-`str` key
degrades the table for good. Compiled reads: `GlobalSlotBase` (one runtime call per
activation, merged by GVN and hoisted by LICM; the frame's globals table when it uses the
site's layout, else an all-unbound array) and `GlobalSlotGet`, whose meaning is the shared
`molt_codegen_abi::global_slot` generator. Stores replace the dictionary row by slot
index (`molt_global_slot_set`). Free-threaded runtimes take the by-name path. The base
becomes inline loads once the frame record exposes the function's globals (stage 1).

**Implementation (method sites, 2026-11-12).** Every `LoadMethod` and the `CallLoadedMethod`
that consumes its whole pair share a site index (`method_sites` in the terminal phase, gated by
the TargetInfo fact `method_sites`: Cranelift, LLVM and WASM). The index names the site's record,
a writable data object the backend emits with the generated image
(`molt_codegen_abi::method_site`): class word, header shape (aux kind and size class), class
layout version, instance-dictionary offset, function, call plan (fixed-arity entry, arity, up to
two trailing defaults) and the attribute name. The record owns its class, function, defaults and
interned name. The meaning is two generators that every backend lowers through its frame builder:

- Load: the receiver is an object; its header shape and masked class word equal the record's; the
  class's layout version equals the record's; its dictionary word, when the class has one, holds
  no dictionary. Hit: `[record.function, receiver]`, both retained. Miss:
  `molt_method_site_load(record, receiver, self_out)`, which interns the name on the site's first
  miss, resolves the method (`object_method_ic_resolve`), checks the instance tier, and fills the
  record when the resolution can repeat (class-inline header, no inferred field of the name, the
  primary GIL runtime, the type epoch stable across resolution). An instance dictionary that does
  not hold the name still sends every load to the miss, which then re-checks only the dictionary.
- Call: `self` is not NULL, the callable is `record.function`, the record has an entry, and the
  call supplies `arity - k` words for some `k` within the record's defaults; the recursion limit
  admits it. Hit: the callable goes into the thread record's invocation word, the entry runs with
  `self`, the arguments and `k` retained defaults, the handoff and depth are restored, and the
  callable is released. Miss: `molt_call_loaded_method<N>`, unchanged.

Guard identities: the class layout version (bumped for the class and every subclass on any
mutation, HF-812) covers rebinding and MRO changes; `__class__` assignment changes the header
class word; an instance attribute creates the dictionary. A user write of a function's binding
metadata or code clears every record's entry for that function (`forget_method_site_call_plans`),
so its calls take the runtime, which reads the live defaults. A free-threaded runtime and an
isolate never fill a record: the record words are plain memory without a lock that orders their
writes with another runtime's reads. Runtime teardown empties every record and releases its edges.
Deleted with the site: `molt_load_method`, the thread-local method cache (`MethodIcEntry`,
`PinnedMethodIcEntry`, `method_ic_lookup`), and every by-bytes method name on the method path; the
zero-argument `super()` load keeps its runtime cache (`SuperIcEntry`). One runtime cache remains
on a site: a load the record cannot answer (a callable stored in an instance field, a descriptor)
takes the generic lookup, whose field tier (`field_sites`) is keyed by the record's address. It
goes when attribute sites land (stage 4, attributes).

**Invariant J3.** A site's cache record is the only cache for that site, and its guard compares
identities that the runtime bumps on every mutation that could change the result (class version,
module slot write, builtins version). Check: mutation tests per guard kind (rebind a global, add a
data descriptor to a class, assign `__class__`) run on native, LLVM and WASM and must observe the
new value at the next execution of the site.

### 4.6 Reference counts

- Values whose TIR type is an immediate carrier (`I64` within the tagged range, `Bool`, `None`,
  `F64` boxed as NaN) get no count operations.
- Other increments are inline: tag test, header load, immortal test (PEP 683), add. Decrements are
  inline with a cold call when the count reaches zero. Under free threading the generated sequence
  compares `ctx.owner_thread_id` with the object's owner and uses the non-atomic local count when
  equal (PEP 703 biased counting); the sequences come from one spec in `molt-codegen-abi` (HF-205).

### 4.7 Scalars in proven loops

Loop-carried values with an `I64` fact live in raw 64-bit carriers. Each add uses the hardware
overflow flag (`adds` + `b.vs`); on overflow the loop exits to a generic version of the same loop
with boxed values (loop versioning; no deoptimization runtime needed). Locals are written to their
frame homes only at observation points: calls to unproven code, raises, and suspension points.

### 4.8 Eval breaker

A poll is `ldr w, [ctx, #eval_breaker]; cbnz w, cold` at loop backedges and after calls that can run
Python code (CPython `CHECK_EVAL_BREAKER`; HotSpot safepoint polls). Producers set the word:
signal handlers, the GC scheduler on allocation thresholds, the async queue, and other threads'
stop-the-world requests. The cold path services the work and clears the word.

**Invariant J4.** Every loop backedge and every return from a call that may run Python code is
followed by a poll before the next backedge; signal latency stays bounded. Check: a CFG verifier
rule in the TIR verify-each pass.

### 4.9 Literals

Each runtime state owns one literal table; a literal load at its use site is a table read
(CPython `co_consts`, HF-507). Revised after one Goldfish round (codebase-details and zoom-out).

**Problem (measured 2026-10-23, HF-507).** Native code builds every heap literal (`str`, `bytes`,
big `int`) in the function prologue on every call and resume, through `molt_string_from_bytes` and a
32-slot direct-mapped cache whose collisions allocate and free literals on each call: a function
with 8 cold `raise ValueError("...")` branches costs 27,693 instructions per call against 3,940
without them (CPython: 1,905 and 1,067). LLVM runs the constructor at every use, inside loops too
(`lowering/constant_ops.rs`); WASM builds stateful and jumpful functions' literals at entry and
others at each use (`function_frame/planning/mod.rs`, `constant_ops.rs`). Payload decoding is
shared (`molt_ir::literal_payload`); object construction is spelled three times.

**Identity.** Every runtime pointer comparison of names has a content fallback, so identity is a
parity question, not a correctness path. Parity rules: identifier-like strings are the runtime's
interned immortal singletons; other literals are one object per module, as CPython merges
constants per module (equal non-name literals in two modules are distinct objects).
`sys.intern(literal) is literal` holds in CPython, so `molt_sys_intern` adopts an already-immortal
literal instead of copying it (today it copies: a parity defect this work fixes).

**Design (stage A, closes HF-507).**
- *Table per runtime state.* Isolates each own a `RuntimeState`, an intern pool and a random hash
  secret, and a `str` caches its hash in the object, so the table lives in the runtime state beside
  `code_slots`, is created at init and is released at teardown; compiled code reaches its base
  through the execution context (HF-198).
- *Dense literal IDs.* The compiler assigns each unique (module, literal) a dense ID and emits the
  payloads as read-only data. The module component is a typed origin, not a name: generated
  bootstrap, chunk, poll and helper bodies today lack a complete originating code identity (lane
  w-startup, 2026-11-07), so each generated body carries its originating module's typed identity
  from the IR, and a body without one is a compile error. `ProgramSlotKind` stays a read-only index
  authority; the writable cells are runtime-owned. Symbol names use a digest of the payload, not its
  hex spelling.
- *One literal-load op.* A new opcode (row in `op_kinds.toml`) loads cell `id`; a zero cell takes one
  cold runtime call that builds the object through the normal allocation and intern path, makes it
  immortal unless it already is (pool, one-character and empty singletons), and publishes it with
  the `canonical_singleton` protocol (build, recheck, only the winner is made immortal, release
  store; the loser releases its copy). The load sits at the use, so a cold branch costs nothing
  until it runs. A `representation_facts` rule keyed on this opcode makes its result
  `BoxedImmortal`: a producer proof, which the RC contract admits (`inline_refcount.md`).
- *Consumers.* Native, LLVM and WASM lower the one opcode; the JIT and the Rust and Luau preludes
  are not affected (they do not consume `const_str`/`const_bytes`/`const_bigint` data segments).
- *Deletions.* Prologue materialization (native `prepare_heap_literals`, WASM entry literal regions)
  and `runtime/molt-runtime/src/const_data_cache.rs` with all its consumers (`builtins/strings.rs`,
  `object/ops_convert.rs`, `state/lifecycle.rs`, `object/immortal_owner_tests.rs`, `lib.rs`). The
  byte constructors stay: the WASM JS hosts and tests call them with transient buffers.

**Gate.** The 8-literal function within 10% of its no-literal variant's instructions on native;
`molt_string_from_bytes` absent from generator-resume samples (HF-229); LLVM and WASM baselines
recorded first (never measured) and the same gate applied; startup instructions not worse; binary
and WASM size recorded; differential tests for interned identity (`is` between identifier
literals, `sys.intern(literal) is literal`) and per-module distinctness.

**Stage B.** Static immortal literal objects at build time are HF-673's first slice (one static-data
emitter for all build-time objects). Go only if stage A's remaining cost (cell test, first-use
allocations, startup) is measurable; the table, its init hook and the cold call (for kinds without
a static layout, such as big integers) remain.

### 4.10 Facts: what exists, what is missing

| fact | status | where |
|---|---|---|
| Stable module function bindings | exists, name-keyed | `frontend/lowering/sema_state.py:_module_stable_funcs` |
| Verified namespace premise over the closure | exists for discovery; frontend adoption open | `docs/design/verified_namespace_premises.md` (HF-226) |
| Callee symbol at a call site | exists as a string | `call_guarded` `metadata.target`; `type_hint="Func:<sym>"` (`frontend/visitors/call_dispatch_named.py:1262`) |
| Function arity, return, closure, definedness | exists, name-keyed maps in the backend | `known_function_arities`, `declared_func_arities`, `function_has_ret`, `closure_functions`, `defined_functions` (`fc/calls.rs`) |
| Op purity and may-raise | exists | `op_kinds.toml` purity tables |
| TIR scalar types | exists | `I64`, `F64`, `Bool`, `BigInt`, `Str`, `Bytes`, `None` |
| Argument custody per call | exists | `argument_custody` op metadata |
| Uses zero-arg `super`/`__class__` | exists | frontend `class_cell` |
| Thread record layout | exists, two fields | `molt-codegen-abi/src/lib.rs:165-168` |
| Object header layout | exists | `molt-codegen-abi` `HEADER_*` constants |
| `NativeSignature` per function | **missing** (pieces are name-keyed) | new typed fact in TIR |
| Call-site binding class (`Proven`/`Guarded`/`Monomorphic`/`Dynamic`) | **missing** as a typed fact | new, from binding flow |
| `GlobalSlot(ModuleId, k)` | exists for global reads and stores (`OpIR::global_slot`, `TirOp::global_slot`); `module.attr` sites open | layouts in the module registry (schema 6); `builtins/namespace_slots.rs`; `molt_codegen_abi::global_slot` |
| `FramePlan` (record or none, home count, class cell) | partial (runtime code-slot frame plan) | move to compile time |
| `FieldSlot(ClassId, j)` for attribute sites | **missing** at compile time (runtime site caches exist) | new, from class layout facts |
| Interprocedural may-raise summary | **missing** | new TIR analysis |
| Location table per code object | data exists on ops (`source_line`, `col_offset`) | new generated table |

### 4.11 What this design refuses

- A pinned context register before measurement (prior art J): `ctx` is an ordinary argument.
- Landing-pad unwinding for Python exceptions (section 4.4).
- Name-matched fast paths (a `fib` or `sum` idiom match): every gain comes from the general
  mechanisms above (HF-185 records that the lost `vec_sum_int_range_iter` idiom hid the loop cost).
- Removing frame or traceback fidelity: every observation keeps CPython's answer (invariant I1).
- A second cache or registry beside an existing one: site records replace runtime-side site caches
  in the same migration.

## 5. Staged plan

Each stage is one complete migration: every backend (native/Cranelift, LLVM, WASM, the JIT, and the
Rust and Luau preludes where they emit the same operations), every runtime consumer, the generated
layout, and the tests. The replaced lane is deleted in the same stage.

### Stage 1: frame records (first stage, concrete)

**Goal.** Replace the runtime-owned frame stack with records in compiled frames. Locations become
plain stores into the record. Frame objects materialize on observation.

**As built (2026-11-11).** Section 4.3 describes the landed design. It differs from the work list
below in four ways: records live in the per-thread frame arena, not in a native stack slot; the
record retains its function and code; the existing `trace_enter_slot` and `trace_exit` ops stay
(the entry's value now carries the frame's home count and clear order) and the shared generators
in `molt-codegen-abi` are their instruction selection on all three backends; and
`frame_context_set` stays for stateful and class-body frames only. The work list is kept as the
record of the original plan.

Measured (VERIFIED: macOS arm64, release, instructions retired from `/usr/bin/time -l`, median of
5 runs, identical output; base is main at 57c433cc8, head adds the inline push and pop):

| program | backend | base | head | change |
|---|---|---:|---:|---:|
| p8 call loop, 10 M calls | Cranelift | 21,007 M | 9,790 M | -53.4% (2,091 to 969 per iteration) |
| p8 call loop, 10 M calls | LLVM | 22,811 M | 11,005 M | -51.8% |
| `fib(32)`, 7.05 M calls | Cranelift | 16,031 M | 8,263 M | -48.5% (2,260 to 1,158 per call) |
| `fib(32)` | LLVM | 14,243 M | 6,116 M | -57.1% |
| `bench_method_default_binding` | Cranelift | 17,698 M | 11,708 M | -33.8% |
| `bench_method_default_binding` | LLVM | 17,884 M | 11,634 M | -34.9% |
| `bench_exception_heavy` | Cranelift | 24,358 M | 24,315 M | -0.2% |
| `bench_exception_heavy` | LLVM | 24,548 M | 24,508 M | -0.2% |

The LLVM rows include the guarded direct call (HF-821 D4) on both sides. Not measured: WASM (it
runs the differential cells, with no instruction row yet) and atomic-refcount builds, where the
record's two retains and each home release are runtime calls.

**Work.**

1. `molt-codegen-abi`: add `FRAME_RECORD_*` layout constants (prev, code, function, location,
   flags, homes base) and `THREAD_RECORD_FRAME_TOP_OFFSET`. Generate the location-table format.
2. Frontend/TIR: compute `FramePlan` per function at compile time (home count, uses class cell,
   suspends). Replace `trace_enter_slot`, `trace_exit`, `frame_context_set`, `line`, and the
   per-op column metadata calls with `frame_push`, `frame_pop` and `set_location(index)` ops
   declared in `op_kinds.toml` as `nothrow`, with their effect on the frame chain. Mark
   `frame_home_store` `nothrow`. Emit the per-code location table.
3. Native backend: allocate the record as a Cranelift stack slot; emit the four push stores, the
   pop store on every exit edge (normal return, exception return, the cold labels), and location
   stores before may-raise or may-observe operations only. Delete the emission of
   `molt_trace_enter_slot`, `molt_trace_exit`, `molt_frame_homes`, `molt_frame_context_set`
   (except functions with a class cell, which store argument zero directly), `molt_frame_set_col`,
   `molt_frame_set_line_col`, `molt_trace_set_line` (emission sites include `function_compiler.rs:1107`,
   `fc/funcobj.rs:108`, `fc/funcobj.rs:1154`, `shared.rs:1120`). The thread record pointer comes
   from the existing once-per-function `molt_thread_record` call until stage 3.
4. LLVM, WASM (shadow stack), JIT and the Rust/Luau preludes (`molt-backend-rust/src/rust/prelude/stateful.rs`
   defines `molt_trace_enter_slot`) emit the same ops.
5. Runtime: delete `FRAME_STACK` and `TRACE_FRAME_PUSH_STACK` (`state/tls.rs`) and migrate their
   consumers (about 148 references in 30 files: tracebacks, `sys._getframe`, frame objects and
   `f_back`, `locals_proxy`, warnings, `module_get_global`'s active globals, `super()` context,
   async poll, generator activation, class init, the C-API hooks) to walk `ctx.frame_top`.
   Materialization and copy-out follow section 4.3. Generators keep their record in the payload.
6. Keep runtime-called Python frames (the generic entry, `call_function_with_arguments`) on the
   same record type: the generic entry pushes a record for the native body it calls, so one chain
   holds both.

**Settled phasing (2026-11-09).** Phase 1a: the `line` op stops costing a runtime call on native,
LLVM and WASM and stays a zero-cost statement marker. It keeps its other roles until a typed
statement-start payload replaces it: the megafunction split boundary
(`molt-tir/src/passes/megafunction_split.rs`), the native RC cleanup point (`fc/funcobj.rs`), the
SSA source-site fallback (`molt-ir/src/tir/ssa.rs`) and the markers the rewrites preserve. Phase
1b deletes `molt_trace_set_line`, its `MOLT_TRACE_LINE*` flags and its WASM manifest row after
HF-699 E2 releases the WASM ABI. Phase 2 makes the frame-home stores exception-free: the TIR
representation facts decide where a raw integer's view must be boxed, and a TIR pass inserts
`frame_home_box` with its exception check there.

**Measurable target.** Call loop (p8) from 3,623 to at most 1,900 instructions per iteration;
`fib` from 4,346 to at most 2,300 per call; method loop from 4,697 to at most 3,000. These budgets
remove the measured frame and location shares (51% of the call loop's samples, 48% of `fib`'s,
39% of the method loop's) and add about 10 instructions of stores. Runtime calls in `add`'s hot
path from 11 to 1 (the once-per-function `molt_thread_record`, removed in stage 3). `add` from 248
to at most 120 static instructions.

**Proof.** The traceback/frame differential families on native, LLVM and WASM; the new random-depth
traceback property test (invariant I1); generator and coroutine suites; the census that no backend
imports the deleted symbols; the 8 programs of this study as benchmark rows on native and WASM;
binary-size and startup comparison (section 6).

**Ledger.** Moves HF-185 frame-cost stages 2, 3 and 6 to done (one migration instead of three);
closes HF-261 (per-statement location calls); moves HF-229 (suspended frames box every value) to
the record-in-payload form.

### Stage 2: thread-record state and inline fast paths

Eval-breaker word and inline polls (J4); inline recursion depth; inline call admission for
`call_guarded` (callee type tag, execution kind and entry pointer read at generated offsets,
replacing `molt_function_direct_call_eligible` and `molt_handle_resolve`); inline reference counts
with immediate elision (section 4.6); `molt_dec_ref_obj`'s per-call header validation moves to debug
builds. Targets: inline-int loop 361 → ≤ 130 (it removes the measured 44% poll and 23% count
shares); call loop → ≤ 1,000; no count operation on an immediate in any of the eight programs.
Closes HF-185 stages 4 and 5, HF-205; moves HF-198.

### Stage 3: compile-time binding and the native calling convention

`NativeSignature` and the two entries (4.2); `ctx` as the first argument on every backend (HF-198
stage 2); module slot arrays and `GlobalSlot` sites (4.5); `Proven`/`Guarded`/`Monomorphic` call
sites; per-state literal tables with dense literal IDs (HF-507, 4.9); `runtime_generation` in the record (HF-200). Deletes
`molt_module_get_global` from module-function call sites, `molt_thread_record`, the
`pending_invocation` handoff, and `molt_string_from_bytes` in prologues. Targets: call loop ≤ 250
instructions per iteration; `fib` ≤ 300 per call. Closes HF-507, HF-200; closes HF-198; moves HF-79.

### Stage 4: inline caches in compiled code

Attribute, method, constructor and module-attribute sites (4.5) with typed name handles.
Typed-layout deallocation: an instance of a fixed-layout class frees by its layout's field list,
without the generic edge-visiting cascade (`DetachedEdgeSink`, `clear_cycle_edges_with_sink`;
31% of the `struct` loop). Deletes runtime-side site caches that the site records replace
(`field_sites.rs` cached fields, `PinnedMethodIcEntry`). Targets (with stages 1-3 landed):
attribute loop ≤ 150; method loop ≤ 350; `struct` (p5) ≤ 600; `struct.pack` ≤ 2,000 (the
remaining cost is the format plan and the wrapper, both cacheable). Closes the HF-185 attribute and
`struct` rows, HF-202; moves HF-206.

Status (2026-11-12): method sites landed (section 4.5). Instructions retired, release, macOS
arm64, minimum of two interleaved runs, output identical, against main fc28795e05:
`bench_method_defaults` (5M calls, a default engaged on every call) LLVM 13.79 to 4.97 billion
(-64%), Cranelift 15.57 to 6.92 billion (-56%); p4 LLVM 10.42 to 1.95 billion (-81%), Cranelift
11.19 to 2.82 billion (-75%). The managed CPython 3.12.16 (-I) retires 8.36 and 4.25 billion; wall
time `bench_method_defaults` 0.14 s (LLVM) against 0.30 s. The method loop is 352 instructions
per call on LLVM, at the stage target before stages 1-3 finish. The recursion check is still two
runtime calls per site call (stage 2), and the module-level store in the benchmark's loop is now
its largest cost (`molt_global_slot_set`, 38% of LLVM self samples). Attribute and constructor
sites remain.

### Stage 5: exceptions

Native exception classes without the C-API detour; lazy tracebacks; class-handle matching (4.4).
Target: raise loop ≤ 1,500 instructions per iteration (CPython 2,038); `type(ValueError.__init__)`
prints `wrapper_descriptor`. These are the accepted stage targets, not current
qualification. HF-589 records the descriptor defect from the historical anatomy;
the refresh below distinguishes the current constructor from that old profile.
Independent of stages 1-4 except for the traceback buffer, which uses the stage 1
record fields `(code, location)`; before stage 1 it can use the existing frame entry.

#### HF-561 refresh and first implementation slice (2026-11-05)

At source `5bdef368edd3`, macOS arm64, logical release with the release-output
runtime, one million runtime-supplied user raises return the exact catch count
in 1.04 s real (0.96 user), against CPython 3.12.16's 0.08 s (0.06 user).
Native instructions are 23,744.456 per operation after subtracting N=0; the
loop/control work remains included. These are single baseline observations,
not a five-pair qualification or an improvement attributed to this slice.
The old C-API-constructor anatomy above is historical: emitted code now calls
`molt_exception_new_builtin_one`. Native exception descriptors already exist.

A five-second wall sample has 3,499 self samples. Structural managed subtype
queries contain 1,191 (34.04%); 889 (25.41% of the total) are descendants of ABI
owner-add/release inside those queries. These overlapping inclusive counts must
not be added. The constructor has 206 inclusive samples, traceback-prefix
capture 316, and frame-column calls 200. A cold first launch spent 93.09 s at
`_dyld_start`; retain that as startup evidence and do not subtract its wall time
from a warm run. It does not explain the steady-state exception gap.

The first change removes temporary tuple ownership from the canonical
`object/class_layout.rs::managed_subtype_relation`. A managed class owns its
immutable MRO. Under the supplied `PyToken` and with no callbacks, use the
existing scoped `seq_access::with_immutable_tuple_slice` to return the scalar
membership result. Exact class handles short-circuit. Nonclass and mixed/native
pairs keep the existing structural type authority and incoming-error behavior.
An unpublished MRO keeps the existing bootstrap traversal; no replacement
hierarchy, cache, native ownership protocol or corruption policy is introduced.
The duplicate metaclass membership scan in `attributes/mutation.rs` migrates to
this same relation. Native, LLVM and WASM consume the shared runtime algorithm.

The borrow cannot escape, allocate, call Python, or release an owner. A live
receiver, descriptor, constructor transaction or builtin table owns each class
through the query; `PyToken` carries current-thread GIL custody. The direct
entries are `try_is_real_subtype` and `is_real_subtype`; their instance-relation,
exception, descriptor, metaclass-construction, constructor, IO and mapping
callers preserve that ownership. Debug builds assert GIL custody. Remaining
MRO-view users compute different facts (first owner, protocol/field policy,
finalizer eligibility, layout sealing, hash or attribute declarations); retain
their existing pins, especially across callbacks. This is not a wholesale
replacement of callback-safe MRO views with borrowed views.

Two sequential independent read-only Goldfish reviews preceded implementation.
The design review found three medium gaps: transient-owner evidence, bootstrap
oracles and wall-cost methodology. The readiness review required closing the
caller/sibling lifetime audit. Responses: audit the actual symbolized scalar
machine-code path and retained sample ancestry for ownership work; net reference
counts prove lifetime balance only. Exercise the same independently specified
class graph before and after MRO publication, including a published C view,
positive/negative/exact/nonclass relations and pending-error identity. The
caller audit above closes the borrow prerequisite. Add no new event counter,
guard or receipt mechanism. The complete private design and review responses
remain in the lane evidence, outside tracked source.

Add `raise_user` and `raise_runtime` (`int("abc")`) to the primitive declaration
and its existing independent CPython output oracle. Preserve old workload bytes
and normalization. Expanded base/head boards must use identical workload identity
and an explicitly selected scratch ratchet, not the old fourteen-row identity.
Run five canonical pairs, with no axis regressing. Separately measure wall slopes
at N=0/K/2K, K=1,000,000, after one retained N=0 warm-up, in five alternating-order
target/CPython pairs. Require positive adjacent slopes agreeing within 10% and
median target/CPython ratios <=3 for both native and the canonical WASM runner;
retain all samples and runner identities. Fixed launch/JIT costs stay separate.

Semantic qualification uses exact CPython 3.12.16 on native, LLVM and WASM:
traceback text/lines, context/cause/finally, nested and bare/explicit reraises,
exception groups/except-star, generators and coroutines. Run direct readers,
whole touched-crate/reverse-dependency library cells in their declared profiles
and features, and one final plain-environment landing precheck. The first slice
alone does not establish the <=3x outcome. Reprofile its changed source before
choosing the next cost. Frame locations and traceback origin/propagation remain
HF-764's reserved authority until it lands; then merge it from main and reuse its
`LocationId`. Do not copy that implementation or create a second location table.

The completed five-pair expanded primitive comparison at `39c629c1a1` measures
the following native instruction slopes against exact CPython 3.12.16. The
codec repair and workload bytes are held constant across base and head.

| Caught raise | Base instructions/op | Head instructions/op | Base/CPython | Head/CPython |
| --- | ---: | ---: | ---: | ---: |
| User `ValueError("x")` | 22,947.928 | 15,585.818 | 11.2881 | 7.6672 |
| Runtime `int("abc")` | 26,318.134 | 21,726.307 | 3.1539 | 2.6084 |

These are diagnostic instruction measurements, not wall-time or release
acceptance. The outlined tuple-membership helper makes no calls. Its caller
invokes it and returns a published-tuple result before the bootstrap pin/drop
paths. A new sample has 2,517 self samples, with
managed ancestry at 69 (2.74%). Traceback prefix capture is 311 (12.36%), of which
308 descend from `record_exception_with_caller_frame`; frame-column work is 181
(7.19%). Terminal retirement is 696 (27.65%), including finalizer policy work.
These inclusive buckets overlap. The next frame capture/location change must
reuse HF-764's authority after it lands. Its branch deletes the old caller-frame
sweep; it was not an ancestor of main `5bdef368edd3` at this measurement.

Declared runtime semantic cells pass in dev, dev-fast and release-fast. Seven
selected exception differentials pass on LLVM and on WASM, including context,
finally, traceback access, `except*`, generator throws and nested reraises.
The native selection exposes invalid-handler `TypeError` values losing their
original exception context. Its current-base cause remains unisolated; the
full exception matrix, traceback text/lines and coroutine obligations remain
unqualified. Dynamic ABI calls per operation remain unknown; eliminating
owner-transition calls from this one path is a different claim.

The expanded primitive comparison reports no regression rows, but it does
report four required improvement updates and dataclass spread 2.382% against
its 1.400% bound. It is not a passing ratchet check. The default ratchet still
has the old workload identity: complete its consumer migration with qualified
calibration before landing the expanded declaration. Do not loosen its bounds
to hide this result. The wall goal is also unqualified: native sampling stops
on a guard custody failure, and the five WASM pairs have a descriptive median
slope ratio 13.0321 but fail the adjacent-slope linearity requirement. Retain
the evidence and resolve these obligations through their existing authorities.

### Stage 6: scalar loops and observation-point locals

Raw `I64` carriers with overflow exits to a boxed loop version; home stores only at observation
points; heap-int frees through a leaf fast path (no edge-visiting cascade for objects without
references). Targets: inline-int loop ≤ 8 instructions per iteration; call loop ≤ 80; attribute
loop ≤ 40; heap-int loop 2,377 → ≤ 400. Closes the loop half of HF-185.

### Stage 7: inlining proven small callees

A TIR inliner inlines `Proven` callees under a size budget. The location table records inline
frames (DWARF inlined subroutines; HotSpot scope descriptors), so a traceback through an inlined
call shows both frames. Target: call loop ≤ 12 instructions per iteration (the loop becomes the
inline-int loop plus the guard-free call body).

### Projected end state

| row | today | after stage 6 [projection] | vs today | vs CPython |
|---|---:|---:|---:|---:|
| Python call (p8) | 3,623 | 40-80 (≤ 12 after stage 7) | 45-90x (300x) | 11-22x faster |
| `fib` per call | 4,346 | 50-100 | 43-87x | 5-10x faster |
| inline-int loop | 361 | 4-8 | 45-90x | 65-130x faster |
| attribute loop | 1,736 | 20-40 | 43-87x | 15-30x faster |
| method loop | 4,697 | 50-100 | 47-94x | 8-17x faster |
| `struct` (p5) | 27,001 | 250-600 | 45-108x | 4-10x faster |
| raise and catch | 210,274 | 800-1,500 | 140-260x | 1.4-2.5x faster |

These are estimates from the instruction budgets in section 4, not measurements. Each stage's
target above is the gate that tests the estimate.

## 6. Cost

**Compile time.** Stage 1 lowers fewer operations (the 20-operation `add` becomes about 8) and emits
fewer call sites (`fib`: 89 → about 15), so Cranelift does less work; the location table is a
linear pass. Stage 3 adds a binding-class decision per call site from facts the binding flow
already computes, and one native plus one generic entry per function (the generic entry already
exists as the trampoline). Stage 4 adds per-site records (data, not code). The interprocedural
may-raise summary is one bottom-up pass over the call graph. Expected net: compile time falls or
stays flat; each stage measures backend time on `tests/differential/stdlib/re_parity.py` before and
after.

**Code size.** Today a location call costs about 20 bytes per site (two `mov`, `adrp`, `add`, `blr`)
and `add` is 992 bytes for `return a + b`. A location store is 8 bytes, a frame push 16, a pop 4, an
inline guard 12-16, an inline increment about 16 with a shared cold call. Stage 1 shrinks compiled
code; stages 2 and 4 grow call and attribute sites by a few instructions each and remove calls.
The runtime dominates binary size (14.0 MB empty; the compiled code of these programs is under
10 KB), so whole-binary size moves little; deleting `FRAME_STACK` machinery shrinks the runtime.
Gate: no stage may grow the empty-program binary or the median compiled function by more than 2%
without a named, measured trade.

**Must not regress.** The differential corpus on native, LLVM and WASM (exact output, including
traceback text and carets); startup (empty program 192.8M instructions, 0.01 s); peak RSS; signal
latency (J4); recursion-limit behavior (`RecursionError` at the configured limit); free-threaded
refcount correctness; generator and coroutine semantics; C-API frame and exception observation.

## 7. Risks and what I could not verify

- **Not measured:** LLVM and WASM backends, Linux and x86_64, the JIT. HF-185 records that LLVM
  lowers `call_guarded` to a runtime call, so LLVM is likely worse on calls.
- **Dynamic call counts:** `lldb` could not run a traced process on this host (120 s without
  progress). Runtime calls per unit come from the hot path read in the disassembly plus sample
  shares, not from counted hits. `MOLT_PROFILE=1` counters exist but count only allocation and
  a few entry kinds. The stage gates need a dynamic count that does not depend on a debugger: one
  counter per exported runtime entry point under the existing `MOLT_PROFILE` authority
  (`state/metrics.rs`), reported per benchmark row as runtime calls per unit. Stage 1 should land
  it first, since it is the oracle for "runtime calls in the hot path".
- **Wall times** ran under load 11-13 on 18 cores; instruction and cycle counts are the primary
  evidence. Short CPython runs (0.06-0.09 s) include about 0.01 s of startup.
- **Projections** in section 5 are instruction budgets, not measurements.
- **Frame observation surface.** Stage 1 must find every reader of the frame stack, including
  C-API hooks and satellite crates bound by symbol name (HF-206). The 148-reference census is a
  `grep` lead, not a proof; the closed `FrameRecord` type makes rustc find the rest.
- **`f_locals` of a live optimized frame** shows values at the last observation point. CPython
  3.13 (PEP 667) shows live values through a write-through proxy. Stage 6 must keep homes current
  at every call to unproven code so a callee's `sys._getframe(1).f_locals` sees CPython's values;
  the property test covers it.
- **Global slot arrays** change the module dict representation; `globals()` identity, iteration
  order and `__dict__` mutation must keep CPython semantics. This is the largest semantic risk in
  stage 3.
- **The C-API detour in exceptions** may also affect other builtin types whose slots resolve to
  CPython-ABI descriptors. I did not survey them.
- The `struct` perf-gate row in HF-185 is object construction, not the `struct` module; this study
  measured both.

## 8. Prior art

- CPython 3.12.11 source (`third_party/cpython-3.12`): `_PyInterpreterFrame`
  (`Include/internal/pycore_frame.h:51`), `_PyFrame_PushUnchecked` (`:261`), lazy frame objects
  (`Python/frame.c:_PyFrame_MakeAndSetFrameObject`, `take_ownership`), `CALL_PY_EXACT_ARGS`
  (`Python/bytecodes.c:2761`), `LOAD_ATTR_INSTANCE_VALUE` (`:1808`), `LOAD_GLOBAL_MODULE`
  (`:1365`), `BINARY_OP_ADD_INT` (`:384`), `CHECK_EVAL_BREAKER` (`Python/ceval_macros.h:117`),
  `COMPUTE_EVAL_BREAKER` (`Python/ceval_gil.c:63`), `_Py_EnterRecursivePy` (`Python/ceval.c:627`),
  traceback at error (`Python/ceval.c:924-941`), `get_exception_handler` (`:1267`), exception tables
  (`Objects/exception_handling_notes.txt`, `Python/assemble.c:assemble_exception_table`),
  vectorcall (`Objects/call.c:_PyObject_VectorcallTstate`, `_PyFunction_Vectorcall`), thread state
  (`Include/internal/pycore_pystate.h:_PyThreadState_GET`).
- mypyc: native `CPyDef_` functions with early binding inside a compilation unit, `CPyPy_` wrappers
  for Python callers, tracebacks added on the error path (`CPy_AddTraceback`).
  https://mypyc.readthedocs.io/en/latest/differences_from_python.html
- Cython: `cdef` C functions, `cpdef` dual entries with an override check, traceback frames created
  only when an exception passes (`__Pyx_AddTraceback`).
- Codon: static compilation of Python to LLVM with monomorphized functions and LLVM exception
  handling.
- PyPy: virtualizable frames whose fields live in registers until something forces the frame.
- HotSpot: monomorphic inline caches with an unverified entry point that checks the receiver class,
  `r15_thread`/`rthread` thread register, safepoint polls, stack guard zones.
- V8: hidden classes and inline caches, `kRootRegister`, interrupt checks at loop backedges.
- Wasmtime: `VMContext` passed to every compiled function; `max_wasm_stack`.
- Cranelift: user stack maps for values live across safepoints; `tail` calling convention (not
  needed: Python has no tail-call elimination and tracebacks need every frame).
- PEP 590 (vectorcall), PEP 683 (immortal objects), PEP 703 (biased reference counting), PEP 669
  (instrument only enabled events), PEP 667 (`f_locals` proxy).

## 9. Programs and reproduction

Each program prints its result, so a run checks its output against CPython's before its count
counts. `p0` is an empty program (`pass`) for the startup subtraction. The long variants (`q*`)
raise the iteration count to make the per-unit share of startup negligible; `q2b` repeats the
`p2` loop 20 times with `total & 1` folded into `grand`, so `total` stays an inline integer. The
small-N variants (`r*`) run 1,000 and 2,000 iterations for the differential check in section 1.

```python
# p1: fib(30)
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


def main():
    print(fib(30))


main()


# p2: counted loop (heap int once total passes 47 bits)
def main():
    total = 0
    for i in range(10_000_000):
        total += i
    print(total)


main()


# p3: attribute store and load
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y


def main():
    p = Point(1, 2)
    total = 0
    for i in range(5_000_000):
        p.x = i
        total += p.x
    print(total)


main()


# p4: method call
class Counter:
    def __init__(self):
        self.step = 1

    def inc(self, v):
        return v + 1


def main():
    c = Counter()
    total = 0
    for i in range(5_000_000):
        total = c.inc(total)
    print(total)


main()


# p5: constructor and two stores (the perf gate's `struct` row)
class Point:
    def __init__(self, x=0, y=0):
        self.x = x
        self.y = y


def main():
    i = 0
    while i < 1_000_000:
        p = Point(0, 0)
        p.x = i
        p.y = i + 1
        i += 1
    print(i)


main()

# p6: struct.pack
import struct


def main():
    total = 0
    for i in range(1_000_000):
        b = struct.pack("<ii", i, i + 1)
        total += len(b)
    print(total)


main()


# p7: raise and catch (HF-561)
def main():
    total = 0
    for i in range(1_000_000):
        try:
            raise ValueError("x")
        except ValueError:
            total += 1
    print(total)


main()


# p8: module function call
def add(a, b):
    return a + b


def main():
    total = 0
    for i in range(10_000_000):
        total = add(total, i)
    print(total)


main()

# x1: exception __init__ descriptor kinds
print(type(ValueError.__init__))
print(type(BaseException.__init__))
print(type(ValueError.__new__))
print(ValueError.__init__ is BaseException.__init__)
print(type(ValueError("x").__init__))
```

Measurement: build each program with `molt build --build-profile release` (`MOLT_KEEP_SYMBOLS=1`
for the disassembly and sampler) and run it, and `python -B -I -S prog.py` for CPython, under
`/usr/bin/time -l`, which reports instructions retired, cycles and peak RSS on macOS. Record the
median of 5 runs; subtract the empty program's instructions and divide by the unit count. The
anatomy, disassembly and sampler steps are those of section 1. Stage 1 turns `p1`-`p8` into
benchmark rows with the instruction budgets of section 5, on native and WASM, so each stage proves
its budget through the benchmark authority (`tools/PERF_AUTHORITY.md`).
