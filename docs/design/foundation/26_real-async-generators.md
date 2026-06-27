<!-- Foundation design 26. Architect: read-only agent, 2026-06-06. Audit + end-state
design for the user mandate: async/generators must be REAL and extreme-performant,
not janky analogues. Saved verbatim per the full-text-artifact policy.
SUPERVISOR CORRECTION (2026-06-06): Phase 0 cites a stale MEMORY.md line claiming the
E1 inliner is dormant. E1 is ACTIVE on native+WASM (7512919fa) and LLVM (0e55aff9a);
the Phase-0 prerequisite is therefore ALREADY MET on all three RC backends (verify the
module-phase path per backend remains the only open check). -->

# Real Async/Generators: End-State Architecture

**Document status:** Design (2026-06-06). Architecture audit + full implementation blueprint.
**Scope:** `def`-with-`yield` generators and `async def` coroutines across all four backends (native/Cranelift, WASM, LLVM, Luau). All claims anchored to current code.

---

## Part 1 — Audit: The Janky Analogue Assessment

### 1.1 What the Frontend Emits

Every `def`-with-`yield` function and every generator expression lower to the same representation: a `_poll` function paired with a heap-allocated frame. The frontend visit is in two paths:

- `visit_FunctionDef` containing yield: `/Users/adpena/Projects/molt/src/molt/frontend/__init__.py` lines ~17695, ~17962 — emits `ALLOC_TASK(poll_func_name, closure_size, metadata={"task_kind": "generator"})`
- `visit_GeneratorExp`: same file lines ~12046 — same `ALLOC_TASK` emission after building the inline `_poll` body
- `visit_AsyncFunctionDef` with yield: lines ~17490 — `STATE_SWITCH` at entry, then body, same `ALLOC_TASK` at the creation site
- `visit_AsyncFunctionDef` without yield (coroutine): lines ~17959 — `ALLOC_TASK(metadata={"task_kind": "coroutine"})`

The `_poll` function receives `self` (the frame pointer) as its sole argument. Its entry begins with `STATE_SWITCH` (a dispatch on `object_state(self)`) that jumps to the current resume block. Every `yield v` becomes a `STATE_YIELD(pair, next_state_id)` that stores the resume-state integer into the frame header and returns the `(v, False)` pair to the caller.

**The heap frame layout** (defined at `/Users/adpena/Projects/molt/src/molt/frontend/_types.py:265-269`):
```
offset 0  (GEN_SEND_OFFSET=0):   send-value slot
offset 8  (GEN_THROW_OFFSET=8):  pending throw-exception slot
offset 16 (GEN_CLOSED_OFFSET=16): bool — generator exhausted
offset 32 (GEN_YIELD_FROM_OFFSET=32): yield-from delegation target
GEN_CONTROL_SIZE=48 bytes of control header
offset 48+: spilled locals at 8 bytes each, deterministic name-sorted order
```

The running/started flags live in `MoltHeader.flags` (`HEADER_FLAG_GEN_RUNNING`, `HEADER_FLAG_GEN_STARTED`), not in the frame payload.

**Spill model** (`_spill_async_temporaries`, lines 7775-7898): a pre-pass over the emitted op list, run at the end of compiling the `_poll` function, that detects every SSA value that is live across a `STATE_LABEL` boundary. For each such value it inserts `STORE_CLOSURE(self, offset, v)` before the `STATE_YIELD`/`STATE_TRANSITION` and `LOAD_CLOSURE(self, offset)` after the matching `STATE_LABEL`. Slot offsets are assigned in alphabetically-sorted name order to be deterministic (to fix non-determinism bug #34 that hash-seeded iteration order created). Every live-across-yield value, including temporaries, gets a dedicated closure slot even if the value has a 1-byte lifetime across the yield point.

**`async def` coroutines** follow the same lowering: they also produce `_poll` + `ALLOC_TASK(task_kind="coroutine")`. The difference is `HEADER_FLAG_COROUTINE` is set on the header and the coroutine participates in `asyncio.Task` scheduling. The frame layout is the same except parameters are stored into `async_locals` slots starting at `async_locals_base` (offset 48 if no closure, 56 if closure-bearing).

### 1.2 Backend Lowering of the State Machine

**StateSwitch in the native backend** (`/Users/adpena/Projects/molt/runtime/molt-backend/src/native_backend/function_compiler.rs:1711-2176`): collects all `state_label` ids into a `BTreeSet<i64>`, pre-allocates Cranelift basic blocks as `resume_blocks`, then emits the state switch as a Cranelift integer switch instruction. Each `state_yield` stores the next-state integer into the frame via `molt_obj_set_state` and jumps to the master return block.

**StateSwitch in LLVM** (`/Users/adpena/Projects/molt/runtime/molt-backend/src/llvm_backend/lowering.rs:3214-3253`): uses `inkwell::builder.build_switch` on `molt_obj_get_state(self_bits)`. The fallback block (for state=0, initial entry) falls through to the entry body.

**StateSwitch in WASM** (`/Users/adpena/Projects/molt/runtime/molt-backend/src/wasm.rs`): threaded through the relooper. `build_dispatch_control_maps` at line 1005 treats `state_label` identically to regular `label` for the relooper shape graph. The WASM relooper models the state dispatch as a structured `br_table` on a loop variable.

**StateSwitch in Luau** (`/Users/adpena/Projects/molt/runtime/molt-backend-luau/src/luau/`): `state_label` emits a Luau label; the state switch becomes an `if/elseif` chain on the state integer.

### 1.3 Runtime Execution Model

**Generator creation** (`molt_task_new` at `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/generators.rs:263-322`): allocates `sizeof(MoltHeader) + closure_size` bytes via `alloc_object`, zeroes all slots to `None`, stores the poll function address, sets state=0. One heap allocation per generator creation. For `closure_size=48+N*8` with N spilled locals, this is a minimum 48-byte + N×8-byte heap object.

**`__next__` / `.send()` dispatch** (`molt_generator_send` at lines 384-494): validates not running/closed, writes the send value and clears the throw slot, swaps in the generator's exception stack and context stack (saving the caller's), calls `call_poll_fn(poll_fn_addr, ptr)`, swaps back the exception/context stacks, handles any pending exception. The context/exception stack swap is the dominant overhead on short-lived generators: it unconditionally saves/restores two `Vec<u64>` stack states and a depth counter on every call.

**`.throw()`** (`molt_generator_throw` at lines 497-588): identical pattern but writes to `GEN_THROW_OFFSET` before resuming.

**`.close()`** (`molt_generator_close` at lines 726-863): injects `GeneratorExit` as a throw, resumes the generator, expects it to return a done pair or raise `GeneratorExit`. If the generator yields a value it raises `RuntimeError("generator ignored GeneratorExit")`. The close path saves/restores exception context exactly like send/throw.

**StopIteration conversion** (`generator_raise_from_pending` at lines 674-723): if a generator body raises `StopIteration`, the runtime wraps it in `RuntimeError("generator raised StopIteration")` with the `StopIteration` as `__cause__`. This is CPython-correct per PEP 479.

### 1.4 Cost Model Per Operation

Measured in distinct runtime operations per `__next__` call on a simple generator:

1. One indirect function call to `call_poll_fn` (via function pointer in the heap header).
2. One `LOAD_CLOSURE` (read `object_state` to dispatch in `STATE_SWITCH`) — the state integer is stored in `MoltHeader.state` (8 bytes in the header).
3. N `LOAD_CLOSURE` calls for each live-across-yield local (one read_64 per slot from the frame payload).
4. Execution of the body until the next `STATE_YIELD`.
5. M `STORE_CLOSURE` calls before each `STATE_YIELD` (one write_64 per slot to be spilled).
6. One `STORE_CLOSURE` to update the state.
7. One `alloc_tuple` for the `(value, done_flag)` pair — a separate 2-element heap tuple, 40 bytes, refcount=1.
8. One `inc_ref_obj` on the pair (line 13642 in `function_compiler.rs`).
9. Return the pair bits to the caller.
10. Caller unpacks the pair: reads `[0]` (value bits) and `[1]` (done flag bits).
11. Caller `dec_ref` on the pair after unpacking.
12. Exception-context stack swap: two `Vec::swap` + depth restore.

For a simple `yield i` in a tight loop this is 12 runtime operations per element, versus 2 for a direct indexed loop (bounds check + load). The frame object (48+ bytes) is allocated on creation and lives for the generator's lifetime. The pair tuple (40 bytes) is allocated and freed per yield. With the RC leak currently present (design 20 drop insertion not yet wired), the pair is currently leaked and accumulates.

### 1.5 Existing Partial Progress

**Deforestation pass** (`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/deforestation.rs`): fuses `sum/any/all/list/len/set/tuple/sorted/reversed(genexpr)` patterns where the body is pure. This handles the common builtin-consumer pattern. It does NOT handle `for x in gen(): ...` loops (impure body check at line 119 rejects `StateYield`, `ClosureLoad`, `ClosureStore`, and `Call`). It also does NOT handle non-builtin consumers.

**`iter_devirt` pass** (`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/iter_devirt.rs`): converts `for x in list` into an index loop, eliminating the iterator object allocation and `IterNext` overhead. This is the proven template the generator fusion pass should follow.

**E1 inliner** (`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/inliner.rs`): splices function bodies at call sites. Currently guards against generator-bearing callees (`has_state_machine()` gate) and exception-handler callees. Provides the `clone_function_body_with_fresh_ids` primitive that generator fusion needs.

**S4 module phase** (`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/module_phase.rs`): the whole-program pass slot. The inliner runs here; generator fusion must run here because it needs both the caller and the `_poll` body simultaneously.

**D1 design doc** (`/Users/adpena/Projects/molt/docs/design/foundation/07_D1-coroelide.md`): a complete, detailed generator fusion blueprint. All recognition predicates, the splice transform, step-by-step algorithm, file-by-file changes, soundness argument, and conservative bail conditions are specified. This document supersedes any prior generator_fusion.md framing.

**Design 20 (RC drop insertion)** (`/Users/adpena/Projects/molt/docs/design/foundation/20_rc-ownership-drop-insertion.md`): specifies the ownership model including §1.3 (generator suspension points — live-across-yield values must be inc-ref'd before yield and dec-ref'd on frame teardown) and §2.9 (the high-level suspension model). The `drop_insertion` pass currently bails on `has_state_machine()` functions (`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/drop_insertion.rs:450`). The path to re-enabling it for state machines is StateSwitch-aware liveness (the dominant borrow interval includes the cross-suspend live range).

**Escape analysis** (`/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/escape_analysis.rs`): marks `AllocTask` as `GlobalEscape` (line ~526 — not directly verified but referenced in design docs consistently). This is correct for escaping generators. The fusion pass bypass this by operating at the recognition stage before escape analysis sees the `AllocTask`.

### 1.6 What Is Absent and Why It Is "Janky"

The "janky analogue" characterization is precise and evidenced:

1. **Every generator is a heap object.** There is no stack-allocation tier, no inline tier. A `sum(x for x in range(10))` that the deforestation fast-path misses allocates a 48-byte heap frame plus a 40-byte pair per element plus a GIL-protected refcount on each. A tight 1M-iteration generator loop creates and destroys 1M pairs at 40 bytes each = 40MB of allocation traffic, none of which is necessary.

2. **Exception context saved/restored on every resume.** `molt_generator_send` unconditionally saves the caller's `ACTIVE_EXCEPTION_STACK` (a `Vec<u64>` clone), swaps in the generator's exception stack, and restores on return — even for generators that never enter a `try` block. This is a CPython-faithful model but not the only correct model. An exception-free generator has no exception state to restore; the swap is wasted.

3. **Pair allocation per yield.** Every `(value, done_flag)` yield produces a fresh 2-tuple. This is the simplest correct return representation but is never necessary after fusion: in a fused loop the pair is scalar-replaced (elem bound directly to the for-target, done flag becomes the loop condition).

4. **State machine blocks are opaque to optimization.** `drop_insertion.rs:450` bails on `has_state_machine()`. This means every generator and coroutine body gets zero RC drops from the TIR pass, relying only on the legacy per-loop heuristic in `function_compiler.rs`. The design 20 §2.9 framework exists but is not wired.

5. **`STATE_SWITCH` is a non-dominator-structured CFG.** The resume blocks are re-entered by the switch dispatch, which violates the dominator-structured assumption every TIR pass is built on. No structural-optimization pass (LICM, GVN, BCE, SCCP, type guard hoist, block versioning, loop unroll, counted loop) fires on generator bodies. The comment at `function.rs:168-204` documents this precisely.

6. **Await inlining is absent.** `async def f(): x = await g()` compiles `g()` to a separate `AllocTask(coroutine)` and then polls it in a loop. There is no mechanism to inline the callee coroutine's poll body into the caller's state machine — the await becomes an `AllocTask` + `STATE_TRANSITION(next_state)` + return pair check + resume on the next `STATE_SWITCH` entry. Each `await` is an indirection through the event loop even when the callee is provably always-ready.

7. **os.walk is deleted from the tree.** The native implementation was reverted at HEAD 934938665 because it was both eagerly allocating all entries (OOM on deep trees) and using native recursion (SIGSEGV on deep trees). The CPython-faithful generator-based os.walk cannot yet be compiled efficiently. This is the single most user-visible symptom.

---

## Part 2 — The End-State Architecture

### 2.1 Generator Tier Ladder

The generator architecture targets four tiers, ordered by escape level. Tier assignment is a compile-time decision made in the module phase; it is not a runtime JIT decision.

**Tier A — Full deforestation (zero allocation, zero dispatch).**
Pattern: `builtin_consumer(genexpr_or_generator)` where the consumer is `sum/any/all/list/len/set/tuple/sorted/reversed` and the generator body is pure or pure-after-inlining. The `deforestation.rs` pass already handles the pure-body case. With the E1 inliner landed, a non-pure body can become pure after callee inlining (e.g. a generator that calls a small pure helper). Tier A does not require generator fusion; it requires the existing `deforestation.run()` to see a pure `ForIter` body.

Required proofs: the generator is non-escaping (single `GetIter` use), the loop body is pure, the consumer is a recognized builtin. All three are currently checked in `deforestation.rs`.

**Tier B — Generator frame elision (stack-like loop-carried phi, no heap frame, no pair allocation).**
Pattern: `for x in gen(): body` where `gen()` does not escape and the generator is not recursive and does not `yield from` into a non-fusable sub-generator.

This is the D1 generator fusion blueprint. The precise splice algorithm is fully specified in `/Users/adpena/Projects/molt/docs/design/foundation/07_D1-coroelide.md` and does not need to be re-derived here. The key structural points:

- The `_poll` function's closure slots become loop-carried phis (block arguments of a `fused_dispatch_block`).
- `STATE_YIELD(pair, next_state)` becomes: bind the element value directly to the for-target, run the consumer body, branch back to `fused_dispatch_block` with updated slot values.
- `STATE_SWITCH(self)` becomes a `Switch` on the `state_phi` block argument.
- `AllocTask`, `GetIter`, `IterNext`/`IterNextUnboxed`/`ForIter` ops are deleted.
- One explicit `IncRef(elem_val)` is inserted at each yield splice point to preserve the `+1` ownership the `IterNext` calling convention delivers.
- After splice, `run_pipeline(caller, tti)` re-runs: SCCP folds the state switch for single-yield generators, LICM hoists loop-invariant loads from the now-inlined body, escape analysis proves the pair is now dead (never allocated), BCE applies on the fused index math.

Required proofs: `AllocTask` has a single `GetIter` use; `GetIter` result has a single `IterNext` use; poll function is available in the same `TirModule`; poll function passes `is_poll_fusable` (no `YieldFrom`, no `StateBlockStart`/`StateBlockEnd`, not in the recursive SCC, `closure_size` statically known); no `.send()`/`.throw()`/`.close()` uses on the generator object.

Tier B is the keystone. It is where os.walk gets fixed.

**Tier C — Stack-allocated frame (escaping-but-non-heap generators).**
Pattern: the generator escapes beyond a single for-loop but does not outlive the current stack frame — it is passed as an argument to a function in the same call chain and consumed before the current function returns.

Escape analysis classifies the `AllocTask` as `ArgEscape` (the `ArgEscape` lattice slot in `escape_analysis.rs` is currently declared but not populated — it is a dead lattice value, as noted in the gap analysis). With `ArgEscape` populated, a non-returning arg use can be lowered to a `StackSlot` in Cranelift instead of a heap allocation via `alloc_object`. The frame's RC ops (set/get state, load/store closure slots) become direct stack pointer offsets. The pair-per-yield still occurs unless the callee is proven to destructure it immediately (which it will be if the callee also undergoes Tier A/B treatment).

Required proofs: `AllocTask` result is `ArgEscape` (or `NoEscape` — Tier B). Needs the `ArgEscape` population of escape analysis, the IP escape summary (E3) to confirm the callee does not store the generator globally, and the SROA pass (E2) for the frame fields if the callee accesses them non-linearly.

This tier is NOT a prerequisite for the os.walk fix (Tier B handles that). It is a polish tier for the remaining escaping-but-local generator patterns.

**Tier D — Heap frame (the current implementation, preserved for true-escape cases).**
Pattern: the generator escapes to a caller (stored in a data structure, returned, passed across a `yield`). The current runtime implementation (`molt_task_new` + poll-fn indirect call) is the final tier. It is correct and must be preserved. The goal is that Tier D only applies when semantically necessary.

### 2.2 Fused Direct-Jump State Machines

The current `STATE_SWITCH` is an integer dispatch table (Cranelift switch, LLVM switch, WASM br_table). For Tier D generators that remain as state machines, the switch is the correct lowering — there is no computed-goto / label-as-value in either Cranelift or standard WASM. The switch dispatch is O(1) after branch predictor training on hot generators.

The structural optimization target at Tier D is: eliminate the `STATE_SWITCH` from the cold path (state=0, first entry) by making the initial entry a direct branch to the init block. For state=0 the switch always takes the first case; a prologue direct-branch optimization (a one-instruction pre-check) eliminates the switch overhead on the hot creation path.

For LLVM specifically, the `state_resume_blocks` at `lowering.rs:3238` already collects resume cases before emitting the switch. An IR-level improvement: emit a direct branch for state=0 before the switch, which LLVM's jump-threading will propagate through indirect call sites.

### 2.3 Async: The Same Ladder + Await Inlining

Coroutines under `asyncio` require two additional structural mechanisms beyond the generator ladder:

**Await inlining.** `await g()` in a coroutine `f` creates a child coroutine `g` and enters a `STATE_TRANSITION` that suspends `f` to wait for `g`. If `g` is statically known and its poll body is available, the await can be transformed by splicing `g`'s poll body directly into `f`'s state machine — exactly the same splice transform as Tier B generator fusion, but applied to `await` rather than `for`. The result: `f`'s state machine handles `g`'s states alongside its own states; no child task is allocated; `g`'s body runs inline in `f`'s poll invocation.

Precondition: `g` passes `is_poll_fusable` (no `YieldFrom`, no exception handlers, not recursive). Additional constraint: `g` must not create its own child awaitables that escape; if `g` calls `await h()` internally and `h` is also inlineable, the splice is applied recursively bottom-up. The bound is the E1 inliner's cost model: the merged state machine must not exceed the per-function instruction budget.

The splice for `await` differs from the `for` splice in one structural detail: there is no "done flag" destructuring — an awaitable poll returns the same `(value, done)` protocol, but `done=False` means "still pending" and the caller's state machine must re-schedule itself (register with the event loop and return), not continue inline. The fusion fires ONLY when the callee is proven to return `done=True` on its first poll for the hot path (trivially true for `asyncio.sleep(0)` whose result is always immediately ready after the wakeup) OR when the entire coroutine is non-I/O (pure computation `async def` that never actually suspends to I/O, which SCCP can prove by tracing the poll-return value).

For the general case, the event-loop contract below handles the remaining awaitables.

**Event-loop contract: allocation-free tasks until genuine I/O suspension.**
The current scheduler (`/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/scheduler.rs`) uses `crossbeam-deque` for work-stealing and a VecDeque ready queue. Each `asyncio.create_task(coro)` allocates a `Task` object that wraps the coroutine and queues it. The path to allocation-free short-lived coroutines is:

A coroutine that completes without yielding to the event loop (i.e., its first `call_poll_fn` returns `done=True`) should never be observed as a `Task` by the scheduler. The call site of `asyncio.create_task` can check the poll-fn return immediately: if done, fulfill the caller's awaiter directly without task allocation. This is the "eager task" optimization present in CPython's `asyncio.ensure_future` implementation when the future is already done. The structural form in molt: `await coro()` where `coro` has no suspension points → lower to a direct call, no `AllocTask`, no event loop involvement. SCCP/Tier B fusion handles this when the poll body is visible and provably terminates in one step.

**kqueue/io_uring-class event loop.** The existing event loop (`event_loop.rs`) uses `mio` for I/O multiplexing on native, which correctly wraps `kqueue` on macOS and `epoll` on Linux. This is the structurally correct implementation. The gap is not the I/O poller itself but the WASM path (documented in design 18: `poll_oneoff` on WASI-preview-1, `block_and_call` on browsers). Design 18 is the structural fix for WASM asyncio; it does not block the native async performance work.

**Structured concurrency soundness.** The `cancellation` module (`async_rt/cancellation.rs`) implements cancel tokens and task-scoped cancellation. This is the correct substrate for Trio-style structured concurrency. The invariant from Nathaniel J. Smith's design: a nursery/task-group must not allow child tasks to outlive the nursery block. The current `TaskGroup` implementation in the stdlib asyncio is Python-level; it works correctly because every `await task_group.__aexit__` waits for all children. The TIR-level concern: inlined `await` must not silently skip cancellation delivery — a generator fusion that removes the `AllocTask` for a child also removes the cancel-token registration point. Guard: the fusion must bail if the callee coroutine's `AllocTask` carries a cancel-token registration (check for `molt_task_register_token_owned` calls in the `_poll` body's preamble).

**asyncio surface-semantic byte-identity.** The asyncio runtime is `_EventLoop` (Python, stdlib) calling `molt_event_loop_*` intrinsics. Generator fusion and await inlining change the internal execution model but must preserve the observable Python semantics:
- Tasks have a `get_result()` / `exception()` surface: preserved because the `Task` wrapper object is only absent for zero-suspension coroutines, which have no observable intermediate state.
- `asyncio.current_task()` must return the Task object for tasks that are running: preserved because a zero-suspension coroutine (fused away) completes atomically and is never "running" from the event loop's perspective — it has no observable window.
- `asyncio.shield()` and `asyncio.wait()` operate on awaitables: preserved because they always receive the outer `Task` object, never the inner coroutine frame directly.

### 2.4 RC Integration with the Generator Ladder

**Tier A (deforestation).** No generator frame exists. RC for loop-body temporaries is handled by the per-function drop insertion pass over the fused loop. No generator-specific RC work needed.

**Tier B (fusion).** After splice, the fused function is no longer a `has_state_machine()` function (no `StateYield`, no `AllocTask`, no `StateSwitch`). The drop insertion pass runs on it normally. The loop-carried phis for frame slots follow the standard phi-ownership model from design 20 §5: each phi at `fused_dispatch_block` takes +1 ownership of its incoming value, drops it on the loop's exit path, and transfers it on the back-edge. This is the standard loop-accumulator ownership pattern that `drop_insertion.rs` already handles for non-state-machine loops.

The one exception: the init values of the frame-slot phis (the values that P's entry block computes before the first `StateYield`). These values were previously spilled to the frame via `ClosureStore` and owned by the frame until yielded back via `ClosureLoad`. After fusion, they become block arguments — the standard phi ownership model applies and drop insertion handles them correctly.

**Tier C/D (stack frame / heap frame).** The `has_state_machine()` bail in `drop_insertion.rs:450` must be replaced with StateSwitch-aware liveness. The algorithm:

A value `v` defined in block `B_def` and used in resume block `B_resume` (reachable only via the `STATE_SWITCH` back-edge) has a live range that includes the suspension interval between `B_def` and `B_resume`. This live range is NOT visible to standard backward dataflow liveness because the `STATE_SWITCH` does not structurally dominate `B_resume` in the control-flow graph. The liveness computation must be augmented:

For each resume block `B_resume` (a `state_label` target), add synthetic pred edges from every block that precedes a `STATE_YIELD(next_state=id_of_B_resume)` into `B_resume`. This makes the "cross-suspend" value visible to the standard backward dataflow. Values that are live across this synthetic edge must be owned by the frame (inc-ref'd before the yield, dec-ref'd on frame teardown via `__del__` / `close()`).

The frame teardown path (`molt_generator_close` → throw `GeneratorExit` → the `_poll` function handles the exit in its `finally` clauses → `_poll` returns `(None, True)`) already triggers the normal function exit path in the `_poll` body. If all frame-owned values are explicit SSA values in the `_poll` body and the drop insertion pass correctly identifies them as live-on-exit, the `DecRef` ops will be placed before the `(None, True)` return. No special frame-teardown pass is needed — the standard drop insertion handles teardown when liveness is computed correctly.

**Zero RC ops in Tier A/B.** The deforestation-fused loop body contains no generator frame accesses. All values are SSA with standard integer/object lifetimes. The drop insertion pass runs on the merged function and inserts exactly the drops that a hand-written equivalent loop would have — no excess. The overflow-peel fast path for integer accumulators runs in the `RawI64Safe` lane with zero drops (the `raw_scalars` filter in `liveness.rs:67` excludes them).

### 2.5 Cross-Backend Structural Requirements

The Tier B splice happens at TIR. TIR is the common representation that feeds all four backends. After fusion the merged function contains:
- Standard `Switch` terminator (existing, all backends support it).
- Standard `CondBranch`, `Branch`, loop structure ops.
- Standard `ClosureLoad`/`ClosureStore` ops for the generator parameters that were captured in the `_poll` body's preamble and are now pre-loop initializations.
- NO `AllocTask`, `StateYield`, `StateSwitch`, `StateTransition`.

Because all four backends already lower TIR with Switch/CondBranch/Branch/ClosureLoad correctly, no backend-specific changes are required for Tier B.

**LLVM StateDispatch substrate (landed).** TIR now models `_poll` re-entry with
first-class `StateDispatch` terminators, and LLVM lowers those terminators to
the real resume blocks with branch arguments. This remains orthogonal to Tier
A/B fusion, which eliminates the state machine before backend lowering, but the
old Tier-D dominance baton is no longer an open prerequisite for generator
`.throw()` resumption.

**WASM relooper constraints.** The WASM relooper models the `STATE_SWITCH` as a loop+`br_table`. After Tier B fusion, the fused loop is a standard structured loop with a `Switch` on the state phi — which the relooper handles as a normal `block` + `br_table`. The WASM path for Tier B is structurally correct without changes to the relooper.

**Luau GC.** Luau is GC-managed; RC drops are no-ops. The fused loop in Luau is a standard Luau `for` equivalent. No RC work in the fused tier.

### 2.6 The itertools/os.walk Consequence

Tier B generator fusion makes hand-written native iterators permanently unnecessary for the recognized shape. The proof:

A Python-idiomatic generator like:
```python
def chain(*iterables):
    for it in iterables:
        for elem in it:
            yield elem
```
compiles to a `_poll` function with two nested loops containing yields. Tier B recognizes `for x in chain(a, b)` and splices `chain`'s poll body into the consumer. Inside the splice, the `for it in iterables` loop becomes a loop-carried phi, and `for elem in it` becomes a nested loop. The result after `run_pipeline`: two nested standard TIR loops, no heap frame.

For **os.walk**: the CPython-faithful implementation is a Python generator that calls `scandir` lazily and yields `(root, dirs, files)` triples. With Tier B, `for root, dirs, files in os.walk(path)` fuses the os.walk generator into the consumer. The `scandir` call is the irreducible I/O boundary — it must remain a call. But the per-entry `DirEntry` objects are immediately destructured (their `name`/`path`/`is_dir` attributes read inline), so escape analysis marks the `DirEntry` as `NoEscape` after fusion and SROA field-promotes it to SSA values. The result is semantically equivalent to the deleted native os.walk implementation with no hand-written Cranelift.

For **itertools**: `chain`, `islice`, `takewhile`, `dropwhile`, `count`, `cycle` (for finite-length inputs), `repeat`, `starmap`, `filterfalse` are all recognizable single-generator patterns. After Tier B landing, the Rust implementations in `runtime/molt-runtime/src/builtins/itertools.rs` can be replaced by Python equivalents. This is the "Hettinger canon" outcome: the stdlib stays Python, and the compiler makes it fast.

**The exact deletion schedule** is: after Tier B is verified on the benchmark set with perf gates confirmed, delete the native itertools implementations one by one, replacing each with a Python generator, verifying byte-identical semantics and perf >= CPython on the benchmark. Do not delete any native implementation until the Python replacement is verified.

---

## Part 3 — Phased Plan, Risk Register, and Gates

### Phase 0 — Prerequisite substrate verification (no new code; verify gates are met)

**What is needed before Tier B:**
- E1 inliner activated on production codegen path (currently dormant — see MEMORY.md "CRITICAL: E1 a+b is DORMANT"). The activation arc (phase-e in the E1 plan) must land first.
- S4 `run_module_pipeline` must be the entry point for production compilation (it is wired for the native backend but the E1 inliner is still not on the prod path).
- The D1 design blueprint (`07_D1-coroelide.md`) is complete and implementation-ready.

**Gate:** E1 inliner producing correct code on `bench_generator_iter.py` and `bench_async_await.py` before fusion is added.

### Phase 1 — Tier B: Generator Frame Elision (D1 implementation)

**Files to create:**
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/generator_fusion.rs` — the recognition + splice pass as specified in `07_D1-coroelide.md` §4.

**Files to modify:**
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/mod.rs` — add `pub mod generator_fusion;`
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/module_phase.rs` — add `run_generator_fusion` call after the E1 inliner in `run_module_pipeline`
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/escape_analysis.rs` — Phase B precision: classify non-escaping `AllocTask` as `NoEscape` for the recognition predicate (Phase A correctness: the fusion pass's own single-use scan is sufficient; escape analysis change is polish)

**Test specifications:**

Differential (byte-identical vs CPython 3.12/3.13/3.14):
1. Simple single-yield generator in a for loop: `for x in gen(100): total += x` — must produce identical output; must show zero `AllocTask`/`StateYield` in TIR dump after fusion.
2. Generator with multiple yields in sequence: `for x in gen_abc(): process(x)` — must preserve yield ordering.
3. `gen.throw(ValueError)` on a non-fused generator (Tier D): throw/catch inside generator body must propagate correctly. Verify Tier B generators receiving throw raise before the for-loop (since `.throw()` is only callable on the generator object, which Tier B eliminates — throw after fusion is a compile-time impossible path; verify the recognition predicate correctly requires no throw uses).
4. `gen.close()` on a fused generator: since Tier B's generator object is eliminated, `close()` is inapplicable — verify the recognition predicate's "no escape" requirement blocks fusion when the generator is passed to a function that calls `.close()`.
5. `finally` inside generator body: `def g(): try: yield 1; finally: cleanup()` — this has `StateBlockStart`/`StateBlockEnd`, which `is_poll_fusable` rejects. Verify bail condition fires and the generator runs correctly via Tier D. The `finally` block must execute on `gen.close()`.
6. Generator raising `StopIteration` internally: must wrap to `RuntimeError` per PEP 479, byte-identical with CPython.
7. Nested generators: `for x in outer(): for y in inner(): body` — both fused independently; verify nested fusion is correct.
8. `send()` value consumed inside generator body: recognition predicate checks `GEN_SEND_OFFSET` reads; if the `_poll` body reads `LOAD_CLOSURE(self, 0)` (send slot), the generator requires `.send()` protocol and must be excluded from fusion. Verify bail.
9. `yield from` inside generator body: `is_poll_fusable` rejects `YieldFrom`. Verify bail and correct Tier D execution.
10. Exception propagation from fused generator body: `def g(): yield might_raise()` — an exception in `might_raise()` inside the fused body must propagate to the consumer's exception handler, byte-identical with CPython. The `CheckException` ops in the cloned body handle this.

Perf gates (all targets, release-fast profile):
- `bench_generator_iter.py` (100 × gen(200)): fused TIR must show no `AllocTask`/`StateYield`. Throughput must be >= CPython and ideally within 2× of a hand-coded index loop.
- No regression on `bench_async_await.py` (async work does not involve generator fusion).
- RSS at exit for a 1M-yield-point generator loop must not grow proportionally (the per-yield pair alloc must be eliminated).

### Phase 2 — Tier D RC: StateSwitch-Aware Drop Insertion

**What changes:** Replace the `has_state_machine()` bail in `drop_insertion.rs:450` with StateSwitch-aware liveness.

The algorithm (per §2.4):
1. Identify all `StateYield(next_state=id)` ops in the function.
2. For each such op, find the corresponding `state_label(id)` resume block.
3. Add synthetic predecessor edges from the `StateYield`-containing block to the `state_label` resume block in the liveness computation.
4. Re-run backward dataflow with these synthetic edges.
5. Values that are live across a synthetic edge are "suspend-live." Suspend-live values must be inc-ref'd before the `STATE_YIELD` and owned by the frame.
6. Run drop insertion normally over the augmented liveness: last-use drops are placed by the standard algorithm; suspend-live values get `IncRef` before each yield that they cross.
7. Frame teardown: the `(None, True)` return block (the exhausted path) sees all suspend-live values as live-in (they are loaded from the frame via `ClosureLoad` at each resume). Standard drop insertion places their `DecRef` there.

**Test specs:**
1. Generator with a heap-value local live across a yield: `def g(): x = SomeClass(); yield 1; use(x)` — x must be inc-ref'd before the yield and dec-ref'd at exhaustion. Verify RSS does not grow on a 1M-iteration version of this.
2. Generator that raises mid-body: the exception path must dec-ref all live-across-yield values. The generator's exception exit returns `(None, True)` via `raise StopIteration` — verify `generator_raise_from_pending` is still correct and the frame is cleaned up.
3. Generator closed via `.close()` with `finally` block: the `GeneratorExit` injection must trigger the `finally`, which runs inside the `_poll` body. All live values at the `finally` entry must have correct RC.

**Perf gate:** The existing `bench_sum.py` and `bench_fib.py` (which run non-generator functions) must show zero regression. The `bench_generator_iter.py` Tier D path (generators that escape and cannot be Tier B'd) must show measured RSS reduction when run with a heap-value-yielding generator.

### Phase 3 — Await Inlining (zero-suspension coroutine elimination)

**What changes:** Extend the `generator_fusion.rs` module with a `run_await_inlining` function. Recognizes `await non_suspending_coro()` patterns.

Recognition predicate for await inlining: the awaited coroutine's `_poll` body contains no `STATE_YIELD`/`STATE_TRANSITION`/`ChanSendYield`/`ChanRecvYield` that route to the event loop — i.e., it always returns `(value, True)` on the first call. This can be proven by SCCP on the `_poll` body: if every control-flow path through the body ends in `return (value, True)` (or equivalently, if the `_poll` body has no `STATE_TRANSITION` ops at all, only `STATE_YIELD` with the exhausted pair), then the coroutine is a one-shot.

For the non-zero-suspension case (the coroutine genuinely suspends on I/O), the await remains as-is.

**Test specs:**
1. `async def f(): x = await pure_coro(); return x` where `pure_coro` has no suspensions — verify `f`'s state machine contains no child `AllocTask` for `pure_coro`; `pure_coro`'s ops are inlined directly.
2. `await asyncio.sleep(0)` — this DOES suspend; verify it is not inlined (bail fires), and the task is correctly rescheduled via the event loop.
3. Exception from inlined awaitable: `await failing_coro()` where `failing_coro` raises — must propagate exception byte-identically to CPython.
4. Cancellation delivery: if `f` is cancelled while awaiting a non-inlineable child, `CancelledError` must be delivered at the `await` point, byte-identical to CPython.

**Perf gate:** `bench_async_await.py` (1000 iterations of `asyncio.sleep(0)`) must show no regression (sleep is a genuine suspension, not inlinable). A new benchmark `bench_async_pure.py` testing `await pure_coro()` must show the fused variant completes without event-loop scheduling.

### Phase 4 — itertools Python Rewrite + Native Deletion

**Precondition:** Phase 1 (Tier B) verified green on the benchmark set including an itertools-consuming test.

**What changes:** For each native itertools implementation in `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/itertools.rs`, replace with a Python generator in `src/molt/stdlib/itertools.py` (which already exists as a Python module — verify its current state). Delete the Rust function one at a time. Each deletion is one commit, verified by a differential test vs CPython.

Priority order: `chain` (simplest), `islice`, `takewhile`, `dropwhile`, `repeat`, `count`, `filterfalse`, `starmap`. The stateful iterators (`cycle`, `zip`, `zip_longest`) are more complex and come later.

**Gate per deletion:** `tests/differential/stdlib/itertools_*.py` suite must be byte-identical vs CPython before and after.

### Phase 5 — os.walk Python Rewrite

**Precondition:** Phase 1 (Tier B) verified. The CPython-faithful os.walk generator implementation (reverted at HEAD 934938665, but the CPython source is known) must compile via Tier B.

**What changes:** Restore os.walk as a Python generator in `src/molt/stdlib/os.py`. The CPython 3.12 implementation:
```python
def walk(top, topdown=True, onerror=None, followlinks=False):
    sys.audit("os.walk", top, topdown, onerror, followlinks)
    return _walk(fspath(top), topdown, onerror, followlinks)

def _walk(top, topdown, onerror, followlinks):
    dirs = []
    nondirs = []
    walk_dirs = []
    try:
        scandir_it = scandir(top)
    except OSError as error:
        if onerror is not None:
            onerror(error)
        return
    with scandir_it:
        while True:
            try:
                entry = next(scandir_it)
            except StopIteration:
                break
            # ... classify entry ...
            if entry.is_dir(follow_symlinks=follow_symlinks):
                dirs.append(entry.name)
                walk_dirs.append(entry.path)
            else:
                nondirs.append(entry.name)
    if topdown:
        yield top, dirs, nondirs
        for new_path in walk_dirs:
            yield from _walk(new_path, topdown, onerror, followlinks)
    else:
        for new_path in walk_dirs:
            yield from _walk(new_path, topdown, onerror, followlinks)
        yield top, dirs, nondirs
```

This has `yield from` (recursive sub-generator delegation). Tier B bails on `YieldFrom`. The correct handling for `yield from` over a same-type recursive generator is a separate recognition pass (Tier B extended). For the initial landing, the Python os.walk compiles via Tier D (heap frame, correct semantics, no OOM/SIGSEGV). The OOM is fixed because `scandir` is now lazy (not an eager `listdir`). The SIGSEGV is fixed because the Python recursion uses the Python call stack with the existing recursion depth guard, not native stack recursion. Tier B fusion of the outer loop body (the `for root, dirs, files in os.walk(path)` consumer) can still fire even though `_walk` internally uses Tier D — the outermost generator frame is fused into the consumer; only the recursive sub-generators use Tier D.

**Gate:** `tests/differential/stdlib/os_walk_*.py` byte-identical vs CPython. RSS < 50MB for a 10,000-directory tree traversal (vs the former OOM). No SIGSEGV on trees of depth > 100.

---

## Risk Register

**R1: `clone_function_body_with_fresh_ids` with ClosureLoad/ClosureStore overrides.**
The D1 blueprint specifies overriding `ClosureLoad` with phi values and collecting `ClosureStore` as yield-point updates. This extends the inliner's clone with new override hooks. If the clone misidentifies a `ClosureLoad` as a frame-slot read (versus a legitimate closure capture from an outer enclosing scope), it will bind the wrong phi. Mitigation: the `_poll` function receives only `self` (the frame pointer) as a parameter. Every `ClosureLoad(self, offset)` in the poll body IS a frame slot read — there are no other closure captures because `self` is the only pointer to any captured state. The outer scope captures are already materialized into the frame slots before the poll body runs. This is verified by the frontend: `_closure_cells_for(free_vars)` packs free variable cells into the frame payload; the `_poll` function reads them via `LOAD_CLOSURE(self, closure_offset + i*8)`. Any `ClosureLoad` with `self` as the first operand is a frame read. Any `ClosureLoad` with a different operand (impossible in a well-formed `_poll` body) is not.

**R2: Multi-yield generator frame-slot initialization.**
For a generator with N yield points, the entry path (state=0) runs through the init block to the first `STATE_YIELD`. Frame slots written AFTER the first yield and read BEFORE the second yield must be correctly identified as "not initialized until yield K". The fusion pass must determine the initial value of each frame slot. If a slot is written only in a resume block (never in the entry path), its initial value is `None` (the frame initializes all slots to `None` in `molt_task_new`). The frame-slot phi's initial value is `None` (an SSA constant). This is conservative-correct: after SCCP propagates the `None` initial value through the first iteration's slot reads, the optimizer will prove the None is only read if the corresponding init code wrote a non-None value first. If the analysis cannot determine the init value, the conservative bail fires (as specified in the D1 blueprint).

**R3: Exception propagation through fused generator body.**
The `CheckException` ops in the cloned body propagate exceptions to the consumer's exception handler. But the exception must be observable as having originated inside the generator (for tracebacks). The current generator exception stack swap (save/restore in `molt_generator_send`) is what provides this — after fusion, the swap is absent. The exception's traceback will show the consumer function's stack frame as the exception origin, not a generator frame. This is observable semantics divergence: Python tracebacks must show the correct origin. Mitigation: this is a traceback-formatting concern, not an exception-routing concern. The exception is correctly raised and propagated; the traceback source location comes from the TIR line-number metadata attached to each op. After fusion, the ops from the `_poll` body carry the original line numbers (preserved by the clone), so the traceback correctly points to the generator function's source line. The `_poll` function's frame is absent (it is not a live activation), but the traceback chain captures the calling convention correctly. This is equivalent to how C compilers eliminate inlined frames from stack traces when `-O2` inlines a callee — the debug info (or here, line metadata) preserves source fidelity.

**R4: send() value on fused generator.**
The recognition predicate requires no `.send()` uses. But the `STATE_SWITCH` dispatch in the `_poll` body dispatches on frame state; it does not read the `GEN_SEND_OFFSET` slot unless the generator body explicitly reads `(yield expr)` as an expression (i.e., uses the send value). The check is: does the `_poll` body contain a `LOAD_CLOSURE(self, GEN_SEND_OFFSET=0)`? If yes, the generator consumes sent values and fusion bails. This covers `g.send(v)` and `x = (yield v)` patterns correctly.

**R5: finally/with blocks in generator body.**
`finally` blocks inside a generator body are `StateBlockStart`/`StateBlockEnd` delimited regions. The `is_poll_fusable` check rejects generators with these (`has_exception_handlers()` returns true for `StateBlockStart`). These generators remain in Tier D. The semantics are: `finally` must run when the generator is closed. This is handled correctly by `molt_generator_close` which throws `GeneratorExit` into the `_poll` body, which triggers the `finally` cleanup. No change needed.

**R6: Reentrancy guard.**
CPython raises `ValueError: generator already executing` if a generator's `__next__` is called while it is executing. The `HEADER_FLAG_GEN_RUNNING` flag in `molt_generator_send` provides this. After Tier B fusion the generator object does not exist, so reentrancy is structurally impossible (there is no generator to call `__next__` on). The recognition predicate's "no escape" requirement ensures the generator object is never exposed; a reentrant call would require the generator object to be accessible, which it is not.

**R7: __del__ timing on Tier D generators.**
When a generator is GC'd without being exhausted (Python's `gen.close()` is called by the runtime in `__del__`), the generator's `finally` blocks must run. For Tier B fused generators, there is no `__del__` to call — the frame is a set of SSA values in the consumer function's stack, and they are correctly cleaned up by the drop insertion pass at function exit. For Tier D generators, the existing `__del__` → `molt_generator_close` path handles this correctly.

**R8: WASM relooper and fused state machine.**
After Tier B fusion, the `Switch(state_phi)` in the fused function is an ordinary switch on a loop variable. The WASM relooper handles this as a `block` + `br_table` (the existing `state_switch` handling). The state_phi is now a standard SSA block argument, not a frame read — the relooper sees a standard loop. No WASM-specific change is needed, but the relooper's existing state-machine support (which handles the non-fused case) must continue to work for Tier D generators. This is guaranteed because the recognition predicate fires only when fusion succeeds; Tier D generators keep their `StateSwitch` and the relooper processes them unchanged.

### Non-Goals

- **Stackful coroutines / greenlets**: not a goal. The current fiber model (poll-fn + heap frame) is the correct Python semantics substrate. Stackful coroutines would require a separate stack allocation per coroutine and are not compatible with the RC ownership model.
- **Parallel generators**: generators are single-threaded under the GIL. The L5 parallel tier (requiring atomic RC, Sam Gross's free-threading work) is a prerequisite for any parallelism; generator fusion is orthogonal and must not introduce race conditions. Verified: Tier B fusion produces a standard TIR loop in the consumer function, which is already GIL-protected.
- **Dynamic specialization (JIT) for generators**: the Tier B decision is AOT. A generator that is fused at compile time is fused for all executions. The deopt skeleton (design 24) can add a runtime guard and bail to Tier D if a dynamic condition (e.g. the generator is stored and used after the loop) is detected, but this is a Y2+ arc.
- **`yield from` general fusion**: `yield from` delegation creates a sub-generator chain that requires recursive fusion. Phase 1 bails on `YieldFrom`. Phase N (unscheduled) handles this when the inner generator is also fusable.

### Dependency Edges

The phases depend on the following landed substrate:
- Phase 1 (Tier B) requires: E1 inliner activated on prod path (E1 phase-e); S4 `run_module_pipeline` on prod path (already wired for the native backend, verify for LLVM/WASM).
- Phase 2 (RC for Tier D) requires: design 20 drop insertion pass (already landed for non-state-machine functions); the StateSwitch-aware liveness extension is the only new piece.
- Phase 3 (await inlining) requires: Phase 1 (generator fusion machinery is shared); the zero-suspension SCCP proof over poll bodies.
- Phase 4 (itertools rewrite) requires: Phase 1 green on itertools-consuming benchmarks.
- Phase 5 (os.walk) requires: Phase 1. Does not require `yield from` fusion for the initial landing.

### Code to Delete at Each Phase

**Phase 1 completion:** After `bench_generator_iter.py` perf gate and all differential tests pass, the native itertools implementations that have Python equivalents become deletion candidates. Do not delete before Phase 4 per-function verification.

**Phase 2 completion:** The `has_state_machine()` bail in `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/drop_insertion.rs:450` is replaced by the StateSwitch-aware liveness path. The bail code is deleted. The `has_state_machine()` predicate on `TirFunction` (function.rs:189) remains valid for other consumers (the inliner, etc.) — it is not deleted, only the drop_insertion bail changes.

**Phase 4 completion (per-function):** Each native itertools function deleted from `/Users/adpena/Projects/molt/runtime/molt-runtime/src/builtins/itertools.rs`. The Rust file may become empty and can then be removed. Any import routing it (`mod itertools;` in `lib.rs` or similar) is cleaned up at the same time.

**Phase 5 completion:** The placeholder os.walk stub (if one exists) is replaced by the Python generator; the native implementation does not exist (it was reverted), so there is nothing to delete from the Rust side.

---

## Benchmark Set Definition

The following benchmarks are the required perf gates for this arc. New benchmarks must be created where they do not yet exist.

Existing (at `/Users/adpena/Projects/molt/tests/benchmarks/`):
- `bench_generator_iter.py` — simple while-yield generator, 100×200 iterations. Gate: Tier B fusion must fire; no `AllocTask` in TIR; throughput >= CPython on all backends.
- `bench_async_await.py` — 1000 `await asyncio.sleep(0)` iterations. Gate: no regression from Phase 1/2 changes; sleep is non-fusable (genuine I/O suspension).
- `bench_range_iter.py` — range() is already devirt'd. Gate: no regression.
- `bench_sum.py`, `bench_fib.py` — non-generator functions. Gate: zero regression from any phase.

New benchmarks to create:
- `bench_generator_chain.py` — `for x in chain(range(500), range(500)): total += x` — exercises multi-generator fusion; gate: fused to a single index loop, RSS flat.
- `bench_generator_exception.py` — generator that raises mid-body in a try/except inside the consumer — verifies exception propagation correctness and performance.
- `bench_async_pure.py` — `await pure_coro()` where `pure_coro` is a no-suspension coroutine — verifies await inlining fires; gate: no event-loop scheduling visible in scheduler counters.
- `bench_os_walk.py` — traverse a synthetic 1000-directory tree; gate: RSS < 50MB, wall time <= 2× CPython, no SIGSEGV.
- `bench_generator_send.py` — `(yield expr)` consumer pattern; verifies the non-fusion bail path doesn't regress performance vs the current heap-frame path.

---

## Key File Reference

Architecture implementation map (files to create or modify, in dependency order):

Create:
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/generator_fusion.rs` (Phase 1, Tier B splice)

Modify:
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/mod.rs` (add `pub mod generator_fusion;`)
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/module_phase.rs` (call `run_generator_fusion` after E1 inliner)
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/drop_insertion.rs` (Phase 2: replace `has_state_machine()` bail with StateSwitch-aware liveness at line 450)
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/liveness.rs` (Phase 2: add synthetic suspend-predecessor edges for StateSwitch-aware liveness)
- `/Users/adpena/Projects/molt/runtime/molt-passes/src/tir/passes/escape_analysis.rs` (Phase 1 polish: `AllocTask` NoEscape for single-use non-escaping frames)

Existing design documents that drive implementation (do not modify, implement from them):
- `/Users/adpena/Projects/molt/docs/design/foundation/07_D1-coroelide.md` — the complete D1 Tier B blueprint
- `/Users/adpena/Projects/molt/docs/design/foundation/20_rc-ownership-drop-insertion.md` — §1.3 and §2.9 for Phase 2

Runtime files whose correctness must be preserved unchanged through all phases:
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/generators.rs` — `molt_generator_send`, `molt_generator_throw`, `molt_generator_close`, `molt_generator_next_method`; these are Tier D runtime and must not be changed by the fusion work
- `/Users/adpena/Projects/molt/runtime/molt-runtime/src/async_rt/event_loop.rs` — asyncio event loop; unchanged by generator fusion

Frontend files whose output the fusion pass consumes:
- `/Users/adpena/Projects/molt/src/molt/frontend/_types.py:265-269` — `GEN_SEND_OFFSET`, `GEN_THROW_OFFSET`, `GEN_CLOSED_OFFSET`, `GEN_YIELD_FROM_OFFSET`, `GEN_CONTROL_SIZE`
- `/Users/adpena/Projects/molt/src/molt/frontend/__init__.py:7775-7898` — `_spill_async_temporaries` (the frame-slot spill logic whose output the fusion pass must correctly parse)
- `/Users/adpena/Projects/molt/src/molt/frontend/__init__.py:12046` and `:17695`, `:17959`, `:17962` — the `ALLOC_TASK` emission sites
---

## Phase 1 — Implementation Findings (2026-06-06)

Phase 1 (Tier B, `tir/passes/generator_fusion.rs`) landed the **single-yield-site,
module-scope** generator-fusion keystone, byte-identical vs CPython 3.12 on
native (all 10 differential tests). The original LLVM evidence was 9/10 because
`t3_throw` predated the StateDispatch closure; the Tier-D `.throw()` gap is now
covered by `tests/differential/basic/generator_throw_resumption.py`. What the
blueprint (parts 07/26 above) assumed and what the Phase-1 lowering did diverged
in three load-bearing ways; the implementation handles all three, and two scoped
sub-cases remain as Finding #1.

### Structural reality vs the blueprint at Phase-1 landing

1. **No explicit resume CFG at TIR in the Phase-1 baseline.** The blueprint
   assumed `STATE_SWITCH → [resume blocks]` was already an explicit TIR `Switch`
   with the frame slots as block-arg phis. The Phase-1 baseline still lowered a
   generator `_poll` as a **linear / structured** TIR body and reconstructed
   resume dispatch in backend lowering. Current Tier-D lowering has since gained
   first-class `StateDispatch`; this finding remains relevant only to why the
   original single-yield-site fusion could be a straight-line interleave.

2. **Frame slots are MEMORY, not phis.** The slots (`closure_load`/`closure_store`
   on the `self` frame pointer, byte offset in the `value` attr) are memory ops,
   not SSA. The fusion pass does the frame-slot **mem2reg itself**: each user slot
   (offset ≥ `GEN_CONTROL_BYTES`=48) becomes a loop-carried phi on the cloned
   poll's own loop header. Param slots (offset = 48 + 8·i for i < AllocTask arity)
   are seeded from the **caller's AllocTask args**; local slots from the poll
   entry-block init `closure_store`. Conservative bail on conditional/multi-block
   slot stores and non-const local inits.

3. **Exception-stack bookkeeping must be elided.** The poll prologue saves the
   exception-stack depth (`exception_stack_enter`/`depth`), restores it before
   every `CheckException` via `Copy(exc_val, exc_val)`, and passes the copies as
   `CheckException` operands; the exception-EXIT block takes the saved values as
   block args on the implicit exception edge. After fusion the generator
   exception stack does not exist: the pass drops the `exception_stack_*` ops + the
   restore-copies (tracked transitively as `exc_derived`), clears `CheckException`
   operands, and strips the exc-stack block args from the exception-exit block
   (the consumer's own `CheckException` reads the runtime pending flag directly and
   carries no operands/edge args). The cloned `label_id_map` is transferred
   (block-key + label-value remapped) so the exception edge resolves.

### Phase-1 Finding #1 — the two scoped extensions that remain

These two cases **bail soundly to Tier D today** (correct, byte-identical via the
heap-frame runtime); both are the SAME underlying mechanism — threading a second
set of values through the fused loop — applied to a different value set.

**(a) Multi-yield-SITE generators** (sequential `yield a; yield b; yield c` — N
`state_yield` ops). The straight-line interleave does not suffice: each yield
must hand control to the consumer body and then RESUME at a *different*
post-yield point, so the consumer body needs a return-dispatch. The structurally
correct form is the blueprint's §3-step-4 model made explicit: partition the
cloned body into yield-delimited segments, build a `fused_dispatch_block(state_phi,
slots…)` whose `Switch(state_phi)` routes to each segment, and route the single
shared consumer body back through the dispatch with the yield's next-state. (For
the common N-sequential-`yield` shape the user-slot set is empty, so this is
return-dispatch with NO mem2reg.) Gate: `apply_fusion` bails when
`yield_count != 1`.

**(b) Function-scope consumers that carry loop state.** A function-scope consumer
threads its own loop-carried values (e.g. an accumulator `total`) as block ARGS
on its loop header — the standard SSA loop-phi form. (Module-scope consumers keep
`total` in the module dict via `ModuleGetAttr`/`SetAttr`, so their loop blocks are
arg-less — which is why module-scope fuses and function-scope does not yet.)
Splicing the generator's loop between the consumer's loop-header edges requires
re-threading those carried values through the fused loop: add them as args on the
generator's loop header alongside the slot phis, seed them from the loop-entry
edge, pass them through yield-pre → consumer-body → post-yield → back-edge.
**`bench_generator_iter.py` is exactly this shape** (a `total` accumulator around
`for val in gen(200)`, nested in a `while outer`), so the perf keystone benchmark
needs extension (b) to fuse. Gate: `apply_fusion` bails when any block in the
consumer loop region carries block args.

Recommended next step: implement (b) first (it unblocks the keystone perf
benchmark and is the simpler of the two — one extra value set threaded through an
already-correct single-yield-site splice), then (a) (the return-dispatch, which
also unblocks doc-26 Phase-1 test 2's "zero AllocTask" TIR-evidence requirement
for sequential multi-yield).

### Resolved #51 follow-up: Tier-D `.throw()` resumption

The earlier Phase-1 notes recorded `t3_throw` (a generator with `try/except`
consumed with `.throw()`) as a pre-existing LLVM state-resume phi failure. That
failure is now superseded by the landed StateDispatch substrate and by the
tracked regression `tests/differential/basic/generator_throw_resumption.py`,
which covers throws before first yield, caught throws after yield, uncaught and
reraised exceptions, `finally`/`close()`, and handler continuation via `send()`.
Keep this test in the native and LLVM differential lanes whenever changing
generator state-machine lowering or the `.throw()` runtime slot protocol.

### Gate evidence (native, release-fast)

- 10/10 doc-26 Phase-1 differential tests byte-identical vs CPython 3.12 (native);
  module-scope single-yield-loop (`counter(n)`-shape) fuses (1 frame elided,
  verified via the `single_yield_in_loop_recognized_and_spliced` unit test:
  zero `AllocTask`/`StateYield`/`IterNext` in the fused caller, `verify_function`
  clean).
- Historical LLVM Phase-1 evidence was 9/10 because `t3_throw` predated the
  StateDispatch closure. Current Tier-D `.throw()` resumption is pinned by
  `tests/differential/basic/generator_throw_resumption.py` and passes native and
  LLVM differential lanes.
- 1126 backend lib tests pass, 0 warnings (all features), no regression.

### Phase-1 perf finding — module-scope fusion is masked by the module-dict accumulator

Measured (native, release-fast, `for x in gen(10_000_000): total += x` at MODULE
scope, the only shape Phase 1 fuses today):
- fused: ~12 s · Tier-D (fusion off): ~12 s · CPython 3.12: 0.89 s.

Fusion correctly eliminates the generator frame (verified: 1 frame elided, no
`AllocTask`/`StateYield`/`IterNext`), but the wall time is UNCHANGED and molt is
~13× SLOWER than CPython — because at module scope `total += x` is
`ModuleGetAttr` + `ModuleSetAttr` per iteration (the ~200× module-dict
accumulator), which dominates and swamps the frame-elision win.
`module_slot_promotion` (which would promote `total` to a loop-carried phi) does
not fire on the freshly-fused loop in this run; whether that is an ordering gap
(fusion runs before module-slot-promotion but the promotion's loop-shape
recognition does not match the fused loop) or a deeper interaction is the first
thing to check.

**Consequence for the perf contract:** the dramatic generator-fusion win (and
"molt faster than CPython" on generator loops) requires the FUNCTION-SCOPE case
(Finding #1b), where `total` is a register local — there the fused loop is a
plain counted loop with a register accumulator, exactly the shape molt already
beats CPython on (`bench_sum` is 0.25 s for the same 10M). `bench_generator_iter`
is function-scope and therefore the right perf gate once #1b lands. Until then,
module-scope generators fuse correctly but show no perf win; this is a known,
documented Phase-1 limitation, not a regression (Tier-D is equally slow, and
`bench_sum`/non-generator loops are unaffected — the pass is a no-op without a
fusable generator).
