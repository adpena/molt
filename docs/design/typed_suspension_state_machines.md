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
- **WASM.** In a C web-server benchmark, a hand-written state machine took 140 ms, Asyncify 700 ms
  and the WasmFX prototype on Wasmtime 2,700 ms
  (OOPSLA 2023, §5, [arXiv 2308.08347](https://arxiv.org/html/2308.08347v3)).
  - The paper counts about 37% of samples in Wasmtime's fiber switch and about 33% in unoptimised
    Rust libcalls.
  - It expects native code generation for suspend and resume to remove much of that.
  - So the figure measures one prototype. It says nothing about JSPI or a production
    implementation.
  - An explicit state machine is the only mechanism that is fast and runs on every engine.
- **Python compilers.** Nuitka first ran generators on fibers (separate C stacks), then replaced
  them with goto-based state machines in 0.5.33 (2018). Generator-using tests ran twice as fast, and
  each generator's stack fell from 2 MB to 1 KB
  ([release notes](https://nuitka.net/posts/nuitka-release-0533.html)).

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

## Host bridge

Ledger: HF-610 (the bridge), HF-611 (virtual threads), HF-612 (stack-switching C
extensions) and HF-614 (`webgpu.h` waits in the browser).

Molt suspends Python code with two mechanisms. They are distinct, and each needs its own host
support.

1. **Coroutine suspension.** Code is `async`/`await`. The typed poll function of this design
   carries it on every host. Only the driver of the poll changes.
2. **Whole-stack suspension.** Synchronous code waits on an asynchronous host: blocking I/O in
   third-party libraries, or a C extension that switches stacks. The poll function cannot carry
   this, because the code is not a coroutine. The host must suspend the native stack.

| Code shape | JS host (browser, Workers, Node) | WASI host | Native |
|---|---|---|---|
| `async`/`await` | the poll, driven by an asyncio loop on the host event loop; promises and awaitables convert both ways | WASI 0.3: the poll lifted as a stackless `callback` export. WASI 0.1/0.2: Molt's own loop over sync imports | the poll on Molt's loop |
| Blocking-style library code | JSPI: suspending imports only where a call truly waits (fetch, sockets over connect, WebGPU map and work-done). Engines without JSPI: typed refusal by default, worker plus `Atomics.wait` proxy as an opt-in | sync imports block the host thread; nothing to suspend | blocks the OS thread |
| C extensions that switch stacks (greenlet) | core stack switching (Phase 3); typed refusal in v1.0 | the same | a Molt-provided switch primitive at the C ABI that saves and restores Molt's per-stack state; greenlet's own assembly is not enough |

### Interleaved stacks are virtual threads

Under JSPI, a host keeps calling promising exports while earlier calls are suspended. Workers runs
many requests in one isolate. So two Python stacks interleave on one OS thread.

That breaks every thread-keyed fact unless the runtime separates them:

- `threading.local`, and through it Django's per-request connections and translations;
- `threading.get_ident()`;
- the active exception;
- the context stack;
- the recursion depth.

**Rule.** Each promising entry runs as a typed **virtual thread**, as Java's virtual threads carry
their own thread-locals ([JEP 444](https://openjdk.org/jeps/444)):

- it gets its own identity and its own thread-local and per-stack state;
- every suspend saves that state and every resume restores it, as a typed record.

Pyodide had to add this save and restore for asyncio state after the fact
([pyodide#4532](https://github.com/pyodide/pyodide/pull/4532)).

The cost is one record swap per suspension. It is measured, not assumed.

### Facts and limits

- **JSPI is Phase 5** ([proposals](https://github.com/WebAssembly/proposals)).
  - It is on by default in Chrome 137 and in Firefox 153, released 2026-07-21
    ([MDN](https://developer.mozilla.org/en-US/docs/Mozilla/Firefox/Releases/153)).
  - Safari has no release note for it.
  - **Workers support is unverified.** The `workerd` repository mentions `Suspending` only in a
    WPT test and a hibernation test. Verify it with `wrangler dev` before the Workers cell claims
    blocking-style code.
- **JSPI traps when a JavaScript frame sits between the suspending import and the promising
  export.**
  - A host entry declared synchronous, such as an event listener or an array callback, can
    therefore never suspend.
  - Molt's host-binding contract declares each entry's kind. The may-suspend analysis refuses, at
    build time, a synchronous entry that can reach a suspending import.
  - Entries the contract cannot see fail with a typed runtime error.
- **WASI 0.3 was ratified on 2026-06-11.** It needs a **component**, while Molt emits WASI 0.1 core
  modules today.
  - Lifting the poll as a `callback` export requires canonical-ABI lifting and WIT bindings.
  - That is a new output format, with its own support-matrix cell.
  - In the Component Model, a callee whose type is not `async` traps if it blocks before
    returning. A sync-lowered caller of an async import suspends the whole component stack, and
    the host carries that suspension (`Concurrency.md`).
- **Cancellation and errors cross the bridge as typed values.**
  - A rejected promise raises a Python exception.
  - An `AbortSignal` cancels the awaiting task with `CancelledError`.
  - An uncaught Python exception rejects the promise.
- **Cost.**
  - Mark only waiting imports as suspending, and keep host crossings coarse
    ([emscripten#21081](https://github.com/emscripten-core/emscripten/issues/21081)).
  - Asyncify is not a default. Emscripten puts its cost at about 50% of time and code size.
  - A selective transform over the proven may-suspend graph needs a measured case and the owner's
    decision.

### Verification

- **One differential suite** runs the same async programs on native, Node (JSPI) and Wasmtime, and
  compares them with CPython.
- **A JSPI case:** `requests.get` against a local server, from a promising entry.
- **An interleaving case:** two concurrent entries that each write `threading.local` and the
  active exception across a suspension; each must read back only its own values.
- **A refusal case:** a synchronous host entry that reaches a suspending import fails at build
  time.
- **Measured:**
  - suspend and resume round trips on V8 and SpiderMonkey;
  - the per-suspension state-record swap.

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

0. **Per-resume runtime cost (HF-230)**: done. Each activation owns its exception state in one
   word of its sidecar, and `ActivationExceptionScope` (`builtins/exceptions/activation.rs`) is
   the one resume protocol of the generator, scheduler, task-poll and `block_on` sites. Entry
   links the caller's innermost handled exception, as CPython's `_PyErr_GetTopmostException`
   walks the `exc_info` chain (HF-495). Poll dispatch resolves a compiled poll without the
   registry lock. Generator loop 300 to 238 ns/op, 9,141 to 7,956 instructions per item. Entry
   and exit still take 11% of a resume, mostly thread-local access to four separate stacks. The
   debug-flag half is HF-F235: every runtime flag is read once, through the generated closed
   type `RuntimeFlag`.
1. **Typed slots and the `ObjOrNull` spill rule**: closes HF-216's remaining item.
2. **The TIR transform** replaces both frontend spill mechanisms (HF-104 becomes a verifier fact).
3. **Locals in SSA within an activation and lazy frame materialization**, shared with HF-185.
4. **Out-word resume protocol** removes the per-yield tuple. Done for a generator's own yields
   (HF-486: generator loop 458 to 313 ns/op, 12,848 to 9,247 instructions per item); the
   yield-from delegate pair remains (HF-229).
5. **Elision** extensions (callee-frame embedding for `await`).
