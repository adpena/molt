# Typed suspension state machines

Status: proposed, 2026-10-15; the frame-state word and the tuple-free yield landed with
HF-486 (2026-10-23); phase 2, slice 1 (the frame split at the TIR lift) landed 2026-11-16. Ledger: HF-229 (transform), HF-230 (per-resume runtime cost), HF-486.
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

Two spill mechanisms chose what crossed a suspension: per-site scratch cells
(`local_bindings.py:365-432`) and the `_spill_async_temporaries` liveness pass
(`async_gen.py:109-197, 1346-1465`). Neither typed its slots, and neither reused a slot. Phase 2,
slice 1 (below) replaced both for values with one TIR pass; scratch cells remain only for break
flags and pattern-match captures. The TIR
verifier rejects a `self_or_null` word in frame storage (`tir/verify.rs:601-616`), so a method call
whose argument can suspend takes the generic call (HF-216). `ClosureLoad` is may-throw and
`ClosureStore` may call Python (`op_kinds.toml:3809-3837`), so check elision and the inliner
refuse stateful bodies (HF-185), and `checked_int_loops` refuses a loop that holds a suspension.

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

## Runtime prior art and verdict (2026-11-16)

A survey of 2024-2026 designs checked the whole async stack, not only the frame layout.

**Verdict.** Keep the stackless split and the Rust loop core. The measured gaps (`await leaf()`
11.9x, `sleep(0)` 2.4x, gather of 10 tasks 17.5x and a 10-item generator about 6x the
instructions of CPython 3.12) come from the work around the state machine.

- Every system that compiles ahead of time and also runs on WASM uses a stackless state machine:
  Rust, C++20, Swift, Kotlin, C#, Nuitka, Cython and mypyc.
- The stackful attempts lost or stalled:
  - .NET's async2 experiment measured stack capture at 48,690 µs GC pauses with 10,000
    suspended tasks, against about 100 µs for compiler state machines. .NET 11 shipped the state
    machine as Runtime Async.
  - Zig 0.16's stackful `Io.Evented` backend is still marked experimental.
  - Go added a separate 20 ns `coroswitch` to make generator-style control flow cheap.
  - WASM stack switching is at Phase 3. WASI 0.3's stackless `callback` export matches the poll
    function.
- io_uring is not ready to adopt. tokio and libuv use it only for files, behind gates. Hosts
  disable it: Google turned it off in production after it featured in 60% of its kernel
  bug-bounty exploit submissions. The completion-based Rust work has moved to compio. Revisiting
  it is an owner decision and needs a measured case.

**What the leaders do that Molt does not, ranked by expected effect.** The attributions are
INFERRED from reading code. Confirm each one with a sampled profile and an interleaved A/B before
ordering the work (`foundation/call_anatomy.md`).

1. **Synchronous start.** An awaited coroutine runs on the caller's stack. Its frame is allocated
   only at its first real suspension, from the split's live set. If may-suspend analysis over the
   closed-world call graph proves it never suspends, it becomes an ordinary call that the
   inliner can inline. CPython also runs `await f()` at once up to the first suspension, so only
   allocation timing and object identity change. The observers of a coroutine that has not yet
   suspended are `frame.f_generator`, `cr_origin` tracking, `gc.get_objects` and
   `inspect.getcoroutinestate` through a frame. A new verified-subset rule for any of them needs
   owner approval. Task scheduling order does not change: this is not eager tasks. Precedents:
   .NET 11 Runtime Async, Cinder eager evaluation, mypyc 1.18. Target: `await leaf()`.
2. **Direct resume.** A proven compiled child is polled by a direct call of its poll symbol, not
   through `molt_future_poll`. Waiter edges belong at the leaf Future only, as in CPython's
   `_asyncio_future_blocking` protocol. Per-resume state becomes the pointer writes of CPython's
   `_SEND_GEN_FRAME`, linked through the frame header (phase 0 below). Target: `sleep(0)` and
   generators.
3. **Stack-ordered frames.** Frames of a suspending await chain come from a per-task bump
   allocator (Swift `TaskAlloc`), or the child frame is embedded in the parent at a fixed offset
   (Rust; Clang `coro_await_elidable`). Recursive call cycles keep the heap. This is phase 5.
4. **Task and Future with CPython 3.14's structures**: a per-thread intrusive task list, the
   current task in the thread record, an inline first callback slot, and one allocation for a
   Task and its fresh coroutine. Today `task_registry.rs` keeps a global `Mutex<HashMap>`. The
   stdlib-source lane owns this (HF-855). Target: gather and free-threading.
5. **One readiness authority, in the loop thread.** Today a separate mio thread polls with a
   250 ms timeout, takes locks and the GIL, and wakes the loop through a pipe. CPython's selector
   loop parks in `selector.select` on the loop thread. `select` (epoll, kqueue, poll) becomes a
   Rust intrinsic, and Windows gets `_overlapped` (IOCP) for the Proactor loop. The separate
   poller and the work-stealing scheduler retire with their last consumer.

**Second pass (papers, WASM tools, more languages, PEPs).** The verdict holds and is stronger:

- Dart's VM copies a frame to the heap only at the first real suspension and creates callbacks
  only at the first `await`, because many async functions never await. dart2wasm replaced its
  JSPI-based async with a state machine: one awaited async call fell from 35,676 ns to 1,082 ns.
  C++26 `std::execution` keeps child operation states inline in the parent, with one allocation
  at the root. Kotlin Flow's `emit` calls the collector directly.
- Item 1 gains a second half: create waiter and callback machinery only at the first real
  suspension. Item 2 gains direct resume of the innermost coroutine of a suspended chain, with the
  chain's running flags as plain stores (C++20 coroutines run about 50x slower on deep
  recursion; .NET 11 resumes the innermost frame directly).
- New item 6: fuse `async for` over a non-escaping async generator, as `generator_fusion` does
  for generators. PEP 525 finalizer timing bounds it: fuse only where early-exit cleanup runs
  where CPython runs it. It ranks after item 4, because no measured gap covers it yet.
- WASM: never route coroutine suspension through JSPI; use JSPI only where code truly waits on
  the host. On JavaScript hosts the host event loop owns time, as Pyodide's WebLoop does (draft
  PEP 818 proposes the same loop for CPython). CPython skips WASI 0.2 for 0.3 (PEP 816).
- Eager task start stays opt-in, as in CPython, Swift (`Task.immediate`) and Kotlin
  (`UNDISPATCHED`). Item 1 is not eager tasks.
- Owner decisions on the verified subset: PEP 669 (`sys.monitoring`) events for elided or fused
  coroutine frames, or a refusal of monitoring for them; what Molt promises for
  `frame.f_generator`, `cr_origin` and `gc.get_objects` before a coroutine object exists; the
  PEP 667 frame-locals write decision.
- PEPs to plan for: PEP 828 (accepted for 3.16, `yield from` in async generators; the frame
  split's delegate mechanism, HF-229, should serve both), draft PEP 852 (an async C API, to build
  on the Rust Future core), PEP 831 (3.15, frame pointers; keep them in LLVM output for profiler
  parity), PEP 568 (deferred; a generator resume saves no context).
- Not adopted: stackful coroutines (Photon's 1.5 ns native switch does not fix memory per
  coroutine, WASM or the coroutine object model), io_uring and WASM stack switching.

Second-pass sources: [Dart VM async](https://mrale.ph/dartvm/async.html),
[dart2wasm state machine](https://dart.googlesource.com/sdk/+/c74387a3a29c6edefd8c423ba7025d298b5b45a0),
[Kotlin Flow](https://kotlinlang.org/docs/flow.html),
[Photon](https://photonlibos.github.io/blog-20241014/stackful-coroutine-made-fast.html),
[P3109](https://open-std.org/JTC1/SC22/WG21/docs/papers/2024/p3109r0.html),
[PEP 789](https://peps.python.org/pep-0789/), and the PEP index read on 2026-11-16
([peps.json](https://peps.python.org/api/peps.json)) for the 3.15 and 3.16 PEP status above.

Sources: [.NET 11 runtime](https://learn.microsoft.com/en-us/dotnet/core/whats-new/dotnet-11/runtime),
[runtime-handled tasks](https://github.com/dotnet/runtimelab/blob/feature/async2-experiment/docs/design/features/runtime-handled-tasks.md),
[Zig std.Io](https://andrewkelley.me/post/zig-new-async-io-text-version.html),
[Go coroutines](https://research.swtch.com/coro),
[Clang PR 99282](https://github.com/llvm/llvm-project/pull/99282),
[WASI 0.3](https://bytecodealliance.org/articles/WASI-0.3),
[component-model concurrency](https://github.com/WebAssembly/component-model/blob/main/design/mvp/Concurrency.md),
[tokio changelog](https://raw.githubusercontent.com/tokio-rs/tokio/master/tokio/CHANGELOG.md),
[Google io_uring restriction](https://openwall.com/lists/oss-security/2023/06/17/2),
[compio](https://docs.rs/crate/compio/latest),
[CPython asyncio internals](https://raw.githubusercontent.com/python/cpython/main/InternalDocs/asyncio.md),
[Scaling asyncio on free-threaded Python](https://labs.quansight.org/blog/scaling-asyncio-on-free-threaded-python),
[mypyc PR 19398](https://github.com/python/mypy/pull/19398).

## Design

The shared repoll operation takes the awaitable and pending-state constant. Its
running state is an attribute and its Ready result is an owned SSA value. A
pending invocation returns without retaining that SSA value. The next dispatch
repolls the saved awaitable, so its Ready continuation receives a fresh result
from that invocation. Terminal ownership exposes both exits before lowering
and owns release of the result, including the exception path. One frame slot
still holds the awaitable itself across polls. Its reloaded owner stays live
through the lexical exception-origin test even after that slot is cleared.


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
   and exit took 11% of a resume, mostly thread-local access to four separate stacks. Those
   stacks now live in the thread record and are reached through the execution context with no
   thread-local read (621769ffe1). The stack pushes remain; direct resume (item 2 of the runtime
   verdict above) replaces them with pointer writes in the frame header. The frame record moved
   first: each activation's persistent record lives in its sidecar, a resume links it and a
   suspension unlinks it inline, and a stateful frame publishes no `super()` context
   (`foundation/call_anatomy.md` section 4.3, "Slice 2": `gen_for` -24.1% instructions on LLVM,
   -23.0% on Cranelift; `bench_generator_chain` -29.9% and -26.3%). What remains per resume,
   ranked by sampled share there: the exception scope (about 24%), the poll call's scaffolding
   (about 12%), and once per activation its completion and death (about 13% of a 10-item
   generator; the largest single cost of `await leaf()`). The
   debug-flag half is HF-F235: every runtime flag is read once, through the generated closed
   type `RuntimeFlag`.
1. **Typed slots and the `ObjOrNull` spill rule**: closes HF-216's remaining item.
2. **The TIR transform** replaces both frontend spill mechanisms (HF-104 becomes a verifier fact).
   Slice 1 landed: [the frame split at the TIR lift](#phase-2-slice-1-the-frame-split-at-the-tir-lift-2026-11-16).
3. **Locals in SSA within an activation and lazy frame materialization**, shared with HF-185.
4. **Out-word resume protocol** removes the per-yield tuple. Done for a generator's own yields
   (HF-486: generator loop 458 to 313 ns/op, 12,848 to 9,247 instructions per item); the
   yield-from delegate pair remains (HF-229).
5. **Elision** extensions: synchronous start, then callee-frame embedding or a per-task frame
   allocator for `await` (items 1 and 3 of the runtime verdict above).

## Phase 2, slice 1: the frame split at the TIR lift (2026-11-16)

Landed: the liveness spill pass left the frontend. One Rust pass,
`molt-passes/src/tir/lower_from_simple/frame_split.rs`, now carries every value that crosses a
suspension, for every backend.

- **Frontend.** A stateful body is lowered like any function: values plus explicit suspensions
  (`state_yield`, `state_transition`) and their resume entries (`state_label`). The frontend
  places no slots for its own temporaries. It writes its frame layout's byte size on
  `state_switch` (`value`). `_spill_async_temporaries`, its homes (`resume_value_homes`) and
  `value_liveness.py` are deleted.
- **The split.** It runs inside the lift (`lower_to_tir`), before CFG and SSA construction:
  1. Liveness over the pre-split graph: each suspension continues at its resume entry, and the
     dispatch has no resume edges. Exception edges stay.
  2. Every entry runs the poll's prologue (the operations before the dispatch). A name the
     prologue defines once is not spilled: a resume reads its own entry's value (the
     exception-stack baseline). A name whose one definition is a constant moves into the prologue
     and keeps one definition. Redefining it after each resume, as a first cut did, gave it a phi,
     and the iterator-pair fusion, which needs its selectors to be unique constants, stopped
     fusing `for x in it` inside a generator. Every other name live into a resume entry gets its
     own slot past the frontend's layout, as LLVM gives each spilled value its own frame field.
  3. A name with one definition is stored once, just after it (and after the exception checks
     that follow it), as LLVM spills at the definition. A loop that suspends then stores nothing
     per iteration for a value defined before the loop. A name defined more than once (a variable
     a loop reassigns) is stored at each suspension instead: a pure suspension stores it just
     before it returns, and a resume entry also reached by fall-through (an `await` re-polls from
     its entry) stores it on that edge.
  4. After the resume entry, its re-entered `try` regions and its frame publication, each name is
     reloaded into the same name. SSA construction joins the reloaded and original definitions.
  This is LLVM `coro.split`'s switched-resume lowering. A first cut stored every live name at
  every suspension, as Kotlin does; that stored a loop's iterators on every yield and cost
  `bench_generator_chain` 19% (LLVM) and 30% (Cranelift) in instructions retired.
- **Source and split forms.** Source IR (a `state_switch` without a `state_targets` map) is the
  pre-split form: its resume edges leave the suspension (`simple_verify::state_resume_op_edges`),
  so the SimpleIR verifier checks that every read after a resume is defined along the suspension
  path. The split writes the explicit map; from then on each resume edge leaves the dispatch, and
  SSA construction refuses a source-form resume edge.
- **Frame size and typed reloads.** The dispatch's `value` stays the frontend layout's byte size,
  where the spill area begins; the split writes the final size as its `task_closure_size`. The
  lift records both (`stateful_spill_base`, `stateful_frame_size`) and `lower_to_simple` re-emits
  them, so a relift never splits again. Type refinement types a reload of a spill slot as the join
  of what the split stores there (the split owns that area): an int loop index stays an int
  across a suspension, so a counted range loop in a generator keeps its unchecked int machinery. The object that compiles the poll
  defines the size as a data symbol (`molt_ir::code_slots::stateful_frame_size_symbol`), and
  each task trampoline (Cranelift and LLVM) loads it. No constructor needs the poll's body, and
  separately compiled objects (batches, the shared stdlib archive) agree through the linker. This
  is LLVM's `coro.size`.
- **Cranelift.** Cranelift consumes `lower_to_simple` of the final TIR. The split's output is the
  explicit form that projection already preserves (`state_switch` with `state_targets`,
  `closure_store`/`closure_load`), so Cranelift needs no lowering of its own. One shared lowering.
- **Why at the lift, not after the optimizer (design item 1).** Every TIR pass between the lift and
  codegen (drop insertion, exception regions, `generator_fusion`, async-work polls) is written
  against the explicit form, and the lift's SSA construction repairs SSA across the reloads for
  free. Moving the split after the optimizer needs those passes to accept the pre-split form; it is
  the next slice, and it brings raw scalar slots (design item 3). `generator_fusion` promotes the
  new slots as it promotes any frame slot.
- **What the frontend stopped doing.** These constructs lower in a stateful body exactly as in
  any function, and the split carries their values: range, index and iterator loops (a range loop
  counts again instead of driving a heap range iterator); augmented assignment, binary operators,
  expression lists and call arguments whose later operands suspend (the callee, the call
  builder, the earlier arguments and `str.split`'s dispatch target and tag, with the exception
  landing that cleared their cells); comparison chains and
  condition joins (PHI); guarded attribute and property loads; parallel assignment; decorators of
  a definition whose signature suspends; `except*`'s rest cell and raised list; the exception value
  of a handler, `finally` or `else`-`finally` block; a return value that crosses a suspending
  `finally` (with it, the `abandon` cleanup that released its scratch cell); the pending exception
  of an `async with` exit; `async for`'s iterator and sentinel; `anext`'s result cell; and
  `yield from`'s delegate iterator (its step pair is a compiler local). A builtin guard may now
  duplicate a region that suspends: each arm's suspensions get their own resume states. The async
  comprehension fills (`_emit_list_from_aiter` and two siblings) had no caller and are deleted.
- **What remains (next slices).** Python locals still live in public frame slots, read and written
  through `closure_load`/`closure_store` (the `gi_frame.f_locals` view reads them); scratch cells
  (break flags, pattern-match captures) still take frame slots; a loaded method's receiver, which
  may be the null sentinel, still enters the call builder before arguments that suspend; `await`
  keeps its awaitable slot and the runtime-written result slot; a slot holds a boxed word, so a
  raw scalar is boxed to suspend (raw slots, design item 3).
- **Spill-slot lifetime.** A slot holds a reference, so the split releases it where its name
  dies: after the operation that reads the name last, and at the head of a block the name is dead
  into although a predecessor holds it live (after a resume entry's reload anchor, so the resumed
  frame is published first). A release is a `closure_store` of None marked `frame_slot_release`;
  spill-slot typing skips it, since no reload reads it. CPython drops a frame value at its last
  use, so a finalizer runs at the same point (`condition_flow_truth_custody.py`, the comparison
  operand that crosses a `yield`). A slot is one name's alone, so a release where the name is dead
  never loses a value a reload needs. A normal return transfers its SSA owner to the caller.
  The frame split clears that value's redundant spill before the adjacent frame unlink and
  return. A block head that defines the name again still keeps the slot until the next store.
- **Measured** (release, macOS arm64, instructions retired, median of 3, e6bac83fe7 against
  8e461d6b13; the table is in `docs/design/foundation/call_anatomy.md` section 4.3): a 10-item
  generator consumed by a `for` loop (`gen_for`) runs 11.3% fewer instructions on LLVM and 14.7%
  fewer on Cranelift; `bench_generator_chain`, `bench_generator_iter` and `bench_async_await` are
  within -1.7% to +0.6%. CPython stays 6.0x ahead on `gen_for`: the remaining cost is per-resume
  runtime work (frame record, activation exception scope, thread-local reads, literal lookups).
- **Differential evidence.** The 167 generator, coroutine, async-generator, `yield from`,
  send/throw/close, frame and exception-across-yield cells of `tests/differential/basic` fail on
  native and LLVM exactly where main (e6bac83fe7) fails (14 programs); no cell fails that passes
  there. Three LLVM cells hit the shared-stdlib refusal (HF-852) once and pass alone. Not
  run: WASM, Linux, Windows, Python 3.13+.
- **Census of what the frontend keeps.** 43 `is_async()` sites remain. Most route public locals
  and the frame's own slots (`local_bindings.py` 10, `function_lifecycle.py` 3,
  `call_dispatch_named.py`, `analysis_collect_static.py` 2), keep scratch cells in the frame
  (break flags and pattern captures: `statement_control_flow.py`, `pattern_match.py`,
  `comprehensions.py` 2), or turn off loop-hoisted guards and module-scope rebinding because a
  suspension can run any code (`loop_lowering.py` 8, `statement_control_flow.py`,
  `call_reductions.py`, `analysis_patterns.py`). One is a workaround with an owner: a range loop
  around `async with` keeps the range iterator (HF-862). The frontend still reserves an
  `await`'s awaitable slot for the runtime's Pending path.

**Completion result ownership (2026-11-27, HF-229 and HF-855).** After lexical cleanup, each
generator or coroutine return uses the normal function return boundary. This removes the
heap cell in the task payload and the read and write of that cell for completion. If cleanup
can suspend, the pending result uses the existing liveness-driven spill, which is cleared
when the return transfers ownership. The shared poll path stores a completion result only
for a future that can replay it. Native coroutines and async generator operations reject
reuse, and coroutine wrappers delegate it. Their completing caller owns the result.
