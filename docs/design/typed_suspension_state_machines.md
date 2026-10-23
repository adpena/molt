# Typed suspension state machines

Status: proposed, 2026-10-15; the frame-state word and the tuple-free yield landed with
HF-486 (2026-10-23). Ledger: HF-229 (transform), HF-230 (per-resume runtime cost), HF-486.
Supersedes the frame-layout and cost sections of
[foundation/26_real-async-generators.md](foundation/26_real-async-generators.md); its generator
elision ladder (tiers A–C) stays the plan for frames that never escape.

## Finding

Molt lowers every generator, coroutine, async generator and generator expression to a poll
function and a heap task. The task payload is the frame. Every Python local and every value that
crosses a suspension lives in that payload as an owned, boxed 8-byte word. Every access is a
runtime call.

| Event | Cost today (code read on main, 2026-10-15) |
|---|---|
| Read or write a local in a stateful body | `LOAD_CLOSURE`/`STORE_CLOSURE`: one runtime entry; an incref on load; incref + decref on store; a raw int is boxed first (`lowering/local_bindings.py:1001-1014`, `function_abi.rs:1837-1877`) |
| `yield` | a `(value, done)` tuple allocation (`async_gen.py:833-903`, `ops_iter.rs:1622-1625`); gone since HF-486, except for the IR iterator pair that `yield from` still builds per delegated item |
| Resume (`molt_generator_send`) | a runtime entry; an exception-stack swap through a thread-local `HashMap`; a context stack behind a global `Mutex<HashMap>` locked twice; 4 incref + 4 decref and a vector push (`generators.rs:449-555`, `exceptions.rs:2575-2584`, `context.rs:46-60`, `frames/namespace.rs:258-288`). Debug flags cost one bit test since HF-F235 (`runtime_flags.rs`); before it, each resume read `MOLT_TRACE_GENERATOR_STATE` from the environment and each WASM poll read two more names |
| `await` | a generic call to `molt_get_awaitable`, `molt_future_poll`, two new frame slots per site |

Two spill mechanisms choose what crosses a suspension: per-site scratch cells
(`local_bindings.py:365-432`) and the `_spill_async_temporaries` liveness pass
(`async_gen.py:109-197, 1346-1465`). Neither types its slots, and neither reuses a slot. The TIR
verifier rejects a `self_or_null` word in frame storage (`tir/verify.rs:601-616`), so a method call
whose argument can suspend takes the generic call (HF-216). `ClosureLoad` is may-throw and
`ClosureStore` may call Python (`op_kinds.toml:3809-3837`), so check elision, `overflow_peel` and
the inliner refuse stateful bodies (HF-185).

## Leading practice

Every leading system stores the values live at each suspension in typed, fixed-offset slots:

- **Rust** computes the locals live across each yield (`MaybeLiveLocals` ∩ `MaybeRequiresStorage`),
  builds a storage-conflict matrix, and lays out saved locals as a prefix plus per-variant
  overlapping fields. "No field ever moves around." The drop shim switches on the state and drops
  only the values in scope there.
  ([coroutine transform](https://github.com/rust-lang/rust/tree/main/compiler/rustc_mir_transform/src/coroutine),
  [layout](https://github.com/rust-lang/rust/blob/main/compiler/rustc_abi/src/layout/coroutine.rs),
  [overlap results](https://tmandry.gitlab.io/blog/posts/optimizing-await-1/))
- **LLVM** finds def-use paths that cross a suspend, rematerializes cheap values, spills the rest,
  and shares slots between allocas whose lifetimes do not overlap. CoroElide moves a frame that
  does not escape into the caller. ([Coroutines](https://llvm.org/docs/Coroutines.html))
- **CPython 3.11+** embeds the frame in the generator. The value stack survives a yield, including
  `attr, self_or_null` with a NULL second word. A `PyFrameObject` exists only when a traceback or
  `sys._getframe()` asks. ([frames](https://github.com/python/cpython/blob/main/InternalDocs/frames.md),
  [generators](https://github.com/python/cpython/blob/main/InternalDocs/generators.md))
- **V8** stores only live registers, and boxes them, because its interpreter must resume any
  frame. An ahead-of-time compiler has no such tier. ([js-generator.tq](https://github.com/v8/v8/blob/main/src/objects/js-generator.tq))
- **Kotlin, C#, Swift** generate one state machine per suspending function and keep only values
  that cross a suspension; .NET Core allocates one box on the first real suspension (about 145 MB
  to 109 KB for 10^6 yields); Swift states the async frame size up front and allocates it from a
  stack-disciplined task allocator.
  ([Kotlin](https://github.com/Kotlin/KEEP/blob/master/proposals/coroutines.md),
  [.NET](https://devblogs.microsoft.com/dotnet/how-async-await-really-works/),
  [Swift](https://github.com/swiftlang/swift/blob/main/stdlib/public/Concurrency/TaskAlloc.cpp))
- **WASM**: a hand-written state machine ran a web-server benchmark in 140 ms, Asyncify in 700 ms,
  and a stack-switching prototype in 2,700 ms (WasmFX, OOPSLA 2023). Stack switching is Phase 3;
  JSPI works on JavaScript hosts only. An explicit state machine is the only fast portable choice.

CPython already keeps NULL words and tagged ints in suspended frame slots, so typed slots add no
semantics that CPython lacks.

## Design

1. **One TIR transform for every backend**, after the main optimizer, so live sets are small and
   facts are final. A suspension becomes: store the live slots, set the state, return. A resume
   becomes: switch on the state, load the slots. The frontend's scratch cells and
   `_spill_async_temporaries` are deleted with it.
2. **Typed live sets per suspension.** Liveness covers all three resume edges (send, `throw()`,
   `close()`); handler and `finally` live-ins are uses. A named Python local stays stored from bind
   to rebind, `del` or exit, so `__del__` timing and `gi_frame.f_locals` match CPython. Temporaries
   and raw values use use-liveness; constants and cheap pure values are rematerialized. Facts that
   depend on mutable state (globals, attribute caches, dict versions) are re-guarded after a resume.
3. **Slot kinds** `Obj`, `ObjOrNull`, `I64`, `F64`, `Bool`, `Tagged`, reusing the two-word
   frame-home ABI (`molt-codegen-abi/src/lib.rs:131-150`). A method pair spills as `Obj` +
   `ObjOrNull`. Pointer slots and raw slots live in separate regions; one generated table maps each
   state to a bitmap of live pointer slots, and the GC traverse, dealloc/close and frame
   materialization read only that table.
4. **Overlapping layout**: Rust's fixed prefix plus per-state variants inside each region. The
   header holds the state word, the exception state, the lazily created frame object and the
   yield-from target. Since HF-486 a generator's state word is a raw atomic `FrameState` at
   payload offset 16, ordered as CPython's `gi_frame_state` (Created < Suspended < Executing <
   Completed < Cleared); entry is one compare-and-swap, which also yields "generator already
   executing", and completion and cycle clearing raise it monotonically. Native coroutines still
   keep two header bits and `TASK_DONE`.
5. **Frame homes as a private alias class that cannot throw.** Only the resume path and frame
   materialization read it, so the optimizer can promote homes to SSA in loops and sink stores to
   the next suspension or frame-observing call. Each call site and suspension carries a frame-state
   map (local name → slot and kind); a `PyFrameObject` is built from it on demand. This is the same
   authority HF-185 needs for synchronous frames; build it once.
6. **Typed resume protocol.** The poll writes its value through an out-word and reports a terminal
   state for completion, as `molt_iter_next_unboxed` already does (HF-217); the per-yield tuple goes.
   Landed (HF-486): the poll's return register is the out-word, and a generator body's normal
   return stores `GENERATOR_RESUME_RETURNED` in its resume label, as a Rust coroutine reports
   `Returned` through its discriminant; the runtime turns the report into `Completed` and resumes
   through one typed step.
7. **Elision.** Keep `generator_fusion` (ZipPy's generator peeling gave 3.58x); require that the
   generator does not escape and that every consumer exit runs `close()` semantics. For
   `await f()` with a known, non-escaping callee, embed the callee frame in the caller frame.
8. **WASM** uses the same machine, dispatching with `br_table` at the loop header so the CFG stays
   reducible.

## Open decision: frame writes

PEP 667 makes `frame.f_locals` a write-through proxy from 3.13. Spec 0216 excludes unrestricted
reflection but does not name frame-local writes. Choose one and write it into the spec before
phase 3:

- (a) the proxy writes the slot and sets a dirty bit; a resume that sees the bit drops facts about
  that local; or
- (b) frame-local writes into compiled stateful frames are outside the verified subset and fail
  loudly; reads stay exact.

Recommendation: (b) for the verified subset, matching the exclusion of `exec`, `eval` and runtime
monkeypatching.

## Verification

- **Fail-closed verifier after the transform**: for every value live at a suspension, a store of
  kind K into slot k precedes it on every path, every resume edge loads kind K from slot k before
  any use, no other value writes k between them, and the state's bitmap marks exactly the live
  pointer slots. After suspensions become returns, SSA dominance proves no value crosses a
  suspension without a slot.
- **Differential**: for each state, drive send, `throw` and `close`, and compare with CPython:
  values, exceptions, traceback lines, `gi_frame.f_locals`, `gi_suspended`, weakref callback order.
- **Sanitizer build**: write a canary into overlapped bytes on each state change and check it on
  reload.
- **Measure** frame bytes, allocations per call, resume latency, native and WASM code size, against
  CPython 3.12+ on `bench_generators`, `bench_async_*`, `bench_class_hierarchy` and HF-185's loops.

## Phases

Each phase is one complete migration across the frontend, TIR, verifier, all backends and the
runtime layout authority.

0. **Per-resume runtime cost (HF-230)**: keep the exception state in the task header, not a
   thread-local map; replace the global context mutex. Independent of the transform; measure
   resume latency before and after. The debug-flag half is done: HF-F235 reads every runtime
   flag once, through the generated closed type `RuntimeFlag`.
1. **Typed slots and the `ObjOrNull` spill rule**: closes HF-216's remaining item.
2. **The TIR transform** replaces both frontend spill mechanisms (HF-104 becomes a verifier fact).
3. **Locals in SSA within an activation and lazy frame materialization**, shared with HF-185.
4. **Out-word resume protocol** removes the per-yield tuple. Done for a generator's own yields
   (HF-486: generator loop 458 to 313 ns/op, 12,848 to 9,247 instructions per item); the
   yield-from delegate pair remains (HF-229).
5. **Elision** extensions (callee-frame embedding for `await`).
