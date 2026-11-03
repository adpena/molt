# Runtime execution context: compiled code carries the attached thread record

Status: proposed design (Elephant draft), 2026-11-03. Awaits integrator review; no runtime code
exists for it. Owner: lane `hf198-entry-context` (design and implementation), integrator (review and
landing). Ledger: HF-198 stage 2. Coordinates with HF-670 (calling convention), HF-205 (inline
refcount), HF-185 (frame records), HF-657 (eval-breaker word) and family B (HF-79 direct calls).
Base: `lane/hf198-entry-context` at `059c9f003d`.

This document is the detailed design of the execution context that
[call anatomy section 4.1](foundation/call_anatomy.md) and
[architecture convergence section C](foundation/77_architecture_convergence.md) require. Those
documents keep their staging; section 11 lists the lines that change when this design is approved.

## 0. Decision in brief

1. **The context is the thread record.** Compiled code already holds one pointer per invocation:
   the address of `ExecutionTls`, which `molt_thread_record` lends (`state/tls.rs`). Stage 2 keeps
   that pointer as the execution context. It adds no second structure.
2. **Runtime entries on the call path take the context as their first argument and skip the
   guard.** These "attached entries" build their `PyToken` from the context. They do not call
   `RuntimeExecutionGuard::enter`, they do not touch the GIL depth, and the entry itself reads no
   thread-local. (A body that uses the frame stack still reads that thread-local until frame
   records land.) Every other runtime entry keeps its guarded path; no entry has both.
3. **The attach transition binds the runtime state once.** The record field that HF-657 added for
   the pending-work word becomes the attached `RuntimeState` pointer. The transition that
   admits execution writes it; the transition that ends execution clears it. Nested entries read
   it with one load. This deletes the per-token state cache (`e458bccd3`) and the per-invocation
   state resolution in `molt_thread_record`.
4. **Two per-call entries become inline record operations.** The recursion charge
   (`molt_recursion_enter_fast`, `molt_recursion_exit_fast`) and the pending-exception test
   become loads and stores at generated offsets on Cranelift, LLVM and WASM. Today only Cranelift
   tests the exception byte inline; LLVM and WASM call `molt_exception_pending` for each check.
5. **The proof is typed.** `AttachedContext<'a>` is the only argument type an attached entry
   accepts. Only attach transitions and the compiled-code ABI produce one. The manifest declares
   each entry's context mode, and a generator projects it into every backend and into rustc-checked
   signature assertions in the runtime.
6. **Compiled-to-compiled calls keep one acquisition per invocation in stage 2.** HF-670 makes the
   context the first parameter of the native entry, where the compiled calling convention changes
   anyway. Doing it here would migrate every compiled entry signature twice.
7. **No pinned register.** The Rust runtime cannot read one, and LLVM and WASM cannot carry one
   here. The gain over an argument is at most one instruction per call.

Projection, not measurement: on the native recursion primitive the change removes about 400 to 500
of about 4,410 instructions per Python call (9-11%); on LLVM and WASM each exception check falls
from a guarded runtime call to a load and a branch. Section 6 is the plan that tests this.

## 1. What exists today

### 1.1 Compiled code already holds the thread record

Stage 1 (`b078d0654a`) put the lease, GIL, drain and runtime-state facts in one const-initialized
thread-local with no destructor. `dbc28f4905` made that struct the thread record compiled code
holds. HF-657 (`d102b4be97`) added the pending-work pointer. The record's ABI prefix is declared in
`molt-codegen-abi` (`THREAD_RECORD_*`) and asserted with `offset_of!` in `state/tls.rs`:

| offset | field | written by | read by |
|---:|---|---|---|
| 0 | `exception_pending: Cell<bool>` | raise, clear, task switch | the runtime; Cranelift inline checks |
| 8 | `pending_invocation: Cell<u64>` | direct-call handoff (inline stores) | the callee's `molt_trace_enter_slot` |
| 16 | `pending_work_ptr: Cell<u64>` | `molt_thread_record`, each invocation | inline polls, all backends |

Each backend takes the record once per invocation:

- **Cranelift** calls `molt_thread_record` in the entry block and keeps the result in a stack slot
  (`function_compiler.rs:493-537`, `:664-679`). The comment there records why it is not a
  `Variable`: SSA repair once dropped the pointer on one edge.
- **LLVM** calls it lazily into the entry block for async polls only
  (`llvm_backend/lowering/value_materialization.rs:1821`).
- **WASM** calls the `thread_record` import in the prologue when the function polls
  (`wasm/function_frame/emission.rs:150-170`) and keeps it in the `__thread_record` local.

So compiled code holds the context already. It does not pass it to the runtime.

### 1.2 What one runtime entry costs

Static counts from the release binary the HF-657 lane built and retained on 2026-11-02
(`logs/agents/lane-hf657-inline-poll/artifacts/anatomy-after/loop_molt`, macOS arm64,
`objdump -d`). The `_tlv_get_addr` thunk is not counted.

| code | non-raising path |
|---|---|
| Guard in a nested entry (`molt_exception_pending_fast`) | 42 instructions outside the body: 4 to reach the `_tlv_get_addr` thunk, 5 to test lease and drain depth, 6 to raise the GIL depth, 12 to build the guard and token on the stack, 15 to drop them |
| `runtime_state_for_gil_in` (main thread, Ready) | 21 instructions, out of line; a token runs it on its first `runtime_state` call |
| `molt_thread_record` | 17 instructions, one thunk call, and `runtime_state_for_gil_in` |
| `molt_recursion_enter_fast` | 20 instructions, two thread-local accesses (one through an out-of-line `call_once`), and `runtime_state_for_gil_in` |
| `molt_recursion_exit_fast` | 12 instructions and one thunk call |

An entry with the same body and a context argument needs no thread-local access, no depth update
and no state resolution. It builds its token in one to three instructions.

### 1.3 The entries one Python call makes

The native call path (call anatomy section 2.1, updated for HF-657, which moved the polls inline):

| step | entry | today |
|---|---|---|
| callee prologue | `molt_thread_record` | thread-local + state resolution |
| frame push | `molt_trace_enter_slot` | guard; `runtime_state` twice |
| homes | `molt_frame_homes` | guard |
| argument zero | `molt_frame_context_set` | guard |
| line | `molt_trace_set_line` | guard |
| line, column | `molt_frame_set_line_col`, `molt_frame_set_col` (several) | `FRAME_STACK` thread-local, no guard |
| frame pop | `molt_trace_exit` | guard |
| caller: global read | `molt_module_get_global` | guard |
| caller: admission | `molt_function_direct_call_eligible`, `molt_handle_resolve` | guard; pure decode |
| caller: recursion | `molt_recursion_enter_fast`, `molt_recursion_exit_fast` | thread-locals + state resolution |
| caller: poll | inline word test (HF-657); cold `molt_async_work_poll_and_exception_pending` | guard on the cold path |
| both: counts | `molt_inc_ref_obj`, `molt_dec_ref_obj` | guard for heap objects |

Seven entries on this path take the full guard on every call. The recursion pair and the record
acquisition resolve the runtime state through thread-locals. That is the "about 15 entries"
of HF-198.

### 1.4 Backend gap: the exception test

Cranelift tests the record's pending byte inline in functions with an exception label. LLVM calls
`molt_exception_pending` for every `check_exception` and `ExceptionPending`
(`llvm_backend/lowering/op_dispatch.rs:458`, `:520`; `value_materialization.rs:325`). WASM calls the
same import for every check (`wasm/op_loop/control_ops/exceptions.rs:77` and six other emitters).
`molt_exception_pending` enters through `with_raw_gil_entry_nopanic!`, so each check on these two
backends pays the guard. Stage 2 fixes the family, not one backend.

## 2. Representation

### 2.1 The context is the thread record

Compiled code holds a pointer to this thread's `ExecutionTls`. I considered two alternatives:

- **A separate per-attachment struct** (HotSpot's frame anchor, a stack-allocated context). The
  facts compiled code needs (pending exception, invocation handoff, recursion depth, pending work)
  are per thread and already live in the record. A second struct would need its own lifetime and
  a pointer from the record. Rejected.
- **The runtime state as the root** (V8's isolate root). Molt runs many threads on one runtime
  state, so thread facts dominate. The record reaches the state with one load, as CPython reaches
  `tstate->interp`. Rejected as the root; kept as the second hop.

### 2.2 Record layout after stage 2

Fields stay fixed-width so one layout holds on every target (native 64-bit and wasm32). The
offsets below are illustrative; `molt-codegen-abi` declares the real ones and `state/tls.rs`
asserts them.

| offset | field | change |
|---:|---|---|
| 0 | `exception_pending: u8` | unchanged; now read inline by all backends |
| 8 | `pending_invocation: u64` | unchanged |
| 16 | `attached_state: u64` | replaces `pending_work_ptr`; the attached `RuntimeState`, written at attach, zero when detached |
| 24 | `recursion_depth: u32` | joins the ABI prefix; was `Cell<usize>` |

`RuntimeState` becomes `#[repr(C)]` with a hot prefix whose layout `molt-codegen-abi` declares:

| offset in `RuntimeState` | field |
|---:|---|
| 0 | `pending_work: AtomicU32` (HF-657's eval-breaker word) |
| 8 | `recursion_limit: AtomicU32` (was `AtomicUsize`) |

Because the pending-work word is at offset 0, `attached_state` equals the old `pending_work_ptr`
value. HF-657's poll emitters keep their loads; they change one constant name. The record holds one
fact (which runtime this attachment runs), not two derived pointers. This mirrors V8's `IsolateData`,
a fixed-layout prefix that generated code reads at constant offsets from the isolate root.

The width change to `u32` follows CPython, whose limit is a C `int`. Phase B checks
`sys.setrecursionlimit` range errors against CPython 3.12 with `recursion_limit.py`.

### 2.3 Where the context comes from at each entry

The context is valid exactly while its thread is attached (invariant J1). Each row names the
transition that binds `attached_state`; compiled code then acquires the record with
`molt_thread_record` (stage 2) or receives it from its caller (after HF-670).

| entry into Python execution | binding transition (existing owner) |
|---|---|
| native `main` | `molt_runtime_ensure_gil` → `ensure_persistent_runtime_execution` (`execution.rs:826`) |
| new Molt thread (shared runtime) | `run_shared_thread` → `RuntimeExecutionGuard::enter_with_worker_cleanup` (`isolates.rs`) |
| isolate thread | `run_isolate_thread` → the bootstrap `with_gil_entry_nopanic!` after `set_thread_runtime_state` |
| C callback on an attached thread (`PyObject_Call`, `PyGILState_Ensure` after `Py_BEGIN_ALLOW_THREADS`) | nested or GIL re-entry: the lease never dropped, so the binding stands |
| C callback on a foreign thread | `PyGILState_Ensure` → fresh `RuntimeExecutionGuard` attach |
| generator, coroutine, async generator resume | the resumer's runtime entry; the poll function acquires the resumer's record |
| asyncio task step | the scheduler's attached entry, same as resume |
| service callback (process wake, signals) | `RuntimeServiceExecutionGuard::try_enter` |
| shutdown drain, retained worker | `ShutdownDrainExecutionCustody::enter` / `for_retained_worker` |
| WASM host call | `molt_runtime_execution_enter` |

A generator or task never stores a context. It runs on whichever thread resumes it, and its poll
function takes that thread's record (invariant J3). CPython does the same: `gen_send_ex2` passes
the resuming `tstate` into the frame evaluation.

### 2.4 How each backend carries it

| backend | within one invocation | to an attached runtime entry | to a compiled callee |
|---|---|---|---|
| Cranelift (and the JIT, which shares its emission) | the existing stack slot; one load per use | first argument, target pointer type | stage 2: the callee acquires; HF-670: first argument of the native entry |
| LLVM | one entry-block value (already done for polls), extended to every function that makes an attached call or an exception check | first argument, `ptr nonnull align 8` | as Cranelift |
| WASM | the `__thread_record` local (i32), set in the prologue | first argument, i32 | as Cranelift |

The Rust and Luau source backends own separate preludes and do not link `molt-runtime`; the MLIR
backend refuses the runtime protocol (convergence document). None of them consume this ABI.

### 2.5 Cost of each choice

Instruction counts are per call on macOS arm64 unless marked.

| choice | compiled → runtime | compiled → compiled | runtime → compiled | other cost |
|---|---|---|---|---|
| **Argument** (chosen for runtime entries) | 1 move; the guard (42 + thunk) and state resolution (21) disappear | 1 move (HF-670) | the caller passes its context | one operand register fewer: AArch64 7 of 8, SysV x86-64 5 of 6, Windows x64 3 of 4 |
| **Pinned register** (Cranelift `enable_pinned_reg`: x21 on AArch64, r15 on x86-64) | still 1 move: rustc code cannot read x21, and stable Rust has no way to bind a parameter to a callee-saved register | 0 | every runtime-to-compiled transfer must load the register first (HotSpot's call stub does this); a C callback needs a stub | one register out of allocation in every Cranelift function; LLVM would need a reserved register per target; WASM has no registers |
| **Reload from the thread-local** (today) | the per-entry guard: about 42 + thunk + 21 | `molt_thread_record`: 17 + thunk + 21 per invocation today; about 12 + thunk after section 3.1 | nothing | compiled code cannot inline a Rust `thread_local!` (the `#[thread_local]` attribute is unstable, so no stable symbol exists), so each reload is a call |

The argument wins for runtime entries by about 42 instructions and a thunk call per entry, and by
21 more when the body resolves the runtime state. For compiled-to-compiled
calls, the argument saves about 15-20 instructions per invocation over the stage-2 acquisition.
That saving belongs to HF-670 (decision 6). The pinned register saves at most one more instruction
per compiled-to-compiled call and costs a register and three divergent backends.

## 3. Typed proof

### 3.1 Runtime side: `AttachedContext<'a>`

```rust
/// Proof that this thread is attached (it holds a lease or drain custody and
/// the GIL) and the address of its record. Only attach transitions and the
/// compiled-code ABI produce one.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub(crate) struct AttachedContext<'a> {
    record: NonNull<ExecutionTls>,
    _attached: PhantomData<&'a ExecutionTls>,
    _not_send: PhantomData<*const ()>,
}
```

- **Producers.** `RuntimeExecutionGuard::context(&self)`, the drain custody with its GIL guard, and
  the first parameter of an attached `extern "C"` entry. A plain `PyToken` cannot produce one:
  a token proves the GIL only (convergence document: "a zero-sized CoreGilToken cannot license an
  unowned RuntimeState pointer").
- **Consumers.** Attached entries: `extern "C" fn molt_trace_exit(cx: AttachedContext<'_>) -> u64`,
  with the body inside `with_attached_entry_nopanic!(cx, py, { ... })` or `with_attached_entry!`.
  The two macros keep the panic contract of `with_gil_entry_body!`: catch under `panic = "unwind"`,
  run directly under `panic = "abort"`.
- **`molt_thread_record`** becomes the acquisition. It reads the thread-local once, checks
  `attached_state != 0` and `gil_depth != 0` (two loads and one branch), and returns the record.
  A failed check aborts with a named message (fail-closed). It no longer resolves or stores the
  runtime state.
- **Hardened builds** assert in `AttachedContext::token` that the record is this thread's
  `execution_tls()` and that it is attached. Release builds trust the type, as CPython trusts the
  `tstate` argument (only its debug build checks, with `_Py_EnsureTstateNotNULL` in
  `_PyInterpreterState_GET`).
- **Runtime-internal callers** of an attached entry call its body function with their token
  instead of the `extern "C"` symbol. The symbol exists for compiled code only.

### 3.2 `PyToken` holds the record; the per-token cache goes

`PyToken<'a>` holds `&'a ExecutionTls` instead of `&'gil GilGuard`. `GilGuard::token` and
`AttachedContext::token` produce the same type, so one token type serves both paths and no body
changes. `runtime_state(py)` reads `record.attached_state` when it is set. It runs the full
resolution only on a token taken outside attachment (lifecycle edges, TLS destruction). This
deletes the `state: Cell<Option<&RuntimeState>>` cache in `PyToken` and the validation that
`runtime_state` performs on each call. Guarded entries gain the same one-load resolution.

### 3.3 Compiler side: the manifest declares the context mode

`wasm_abi_manifest.toml` is the target-neutral runtime-function ABI authority (convergence
document). Each `[[import]]` row gains `context = "attached"` or keeps the default
(guarded). `tools/gen_wasm_abi.py` projects it:

- **Machine signature.** The generator prepends the context (target pointer on native, i32 on
  WASM) to an attached row's operand signature. The row's `type` names the operand signature only.
- **Rules the generator enforces.** An attached row has no `callable_arity`: an attached entry is
  never a Python callable, because the callable trampoline holds no context. `trace_enter_slot`,
  `trace_exit` and `trace_set_line` carry `callable_arity` today, and no Python source calls them
  (`src/molt/_intrinsic_symbols.py` registers them only); Phase B removes their callable rows
  through the intrinsic manifest.
- **`molt-ir` projection.** `RuntimeFn::context_mode()`, so backends dispatch on a closed type.
- **Backends.** Each backend wraps its context value in a newtype (`ExecCtx(Value)` on Cranelift,
  `ExecCtx(PointerValue)` on LLVM, `ExecCtx(LocalIndex)` on WASM) that only the acquisition (and
  later the HF-670 parameter) constructs. Attached calls go through one emitter per backend,
  `emit_attached_call(ctx: ExecCtx, f: RuntimeFn, args)`. The generic import path refuses an
  attached symbol, so an attached call without a context is a compiler failure, not a wrong
  machine signature.
- **Runtime projection.** A generated module in `molt-runtime` coerces each attached entry to its
  manifest signature, for example
  `const _: for<'a> extern "C" fn(AttachedContext<'a>) -> u64 = crate::molt_trace_exit;`.
  rustc then rejects a Rust definition whose context mode disagrees with the manifest.

## 4. Invariants and their producers

| id | invariant | producer | check |
|---|---|---|---|
| J1 attachment | An `AttachedContext` is used only while its thread holds a lease or drain custody and the GIL. A GIL release inside an attached entry pauses that use until the entry re-acquires the GIL (J3). | The attach transitions in section 2.3; `molt_thread_record`'s check for compiled code. | Rust: no other constructor. Hardened assertion in `token()`. Unit test: `molt_thread_record` on a detached thread fails closed with its message (a panic under the test profile's `panic = "unwind"`). |
| J2 isolate resolution | `attached_state` is non-zero exactly while the thread holds a lease or drain custody. It equals `runtime_state_for_gil_in` once the custody is complete, and it stays constant until the last release. | The custody constructors, each after it has established all of its facts: `RuntimeExecutionGuard` after admission, the service guard after its origin borrow, `ShutdownDrainExecutionCustody::enter`, and `for_retained_worker` after it records the origin. The raw depth increment is too early: `for_retained_worker` records `retained_worker_origin` after `enter` raises the drain depth (`execution.rs:235-248`), and during `Finalizing` the resolution needs that origin. Every writer of `tls.runtime_state` (isolate start, shared-thread start, worker start in `run_runtime_worker`, process callback, runtime init and teardown) runs before custody; Phase B confirms each. | Hardened: `token()` compares `attached_state` with the full resolution. Unit tests: isolate thread, shared thread, process callback, service callback, finalizing owner, retained worker. |
| J3 suspension and resume | No heap object stores a context: not a task payload, closure, frame, function object or generator. A GIL release inside an attached entry keeps the record and its binding; the entry re-acquires the GIL before it returns to compiled code. | Backend `ExecCtx` newtypes have no store operation except Cranelift's per-invocation spill slot. `GilReleaseGuard` saves and restores the whole GIL depth (`gil.rs:452`). | Differential: a generator created on the main thread and resumed on a worker thread, with a raise and a recursion-depth probe; `time.sleep` in a callee while another thread runs Python. |
| J4 re-entrant C callbacks | C code never receives a context. A C-API boundary enters through `RuntimeExecutionGuard`: nested on an attached thread (same record, same binding) or a fresh attach on a foreign thread. | The C-API boundary in `molt-cpython-abi` and `cpython_abi_hooks.rs`. | Existing unit test `suspended_gil_entry_inherits_ordinary_and_shutdown_custody`, plus a new foreign-thread `PyGILState_Ensure` test. Differential with a `ctypes` callback if that cell is supported. The C-extension execution cell is blocked by HF-690. |
| J5 shutdown drain | During finalization, Python runs only under drain custody or the finalizing owner's retained lease; both bind. A `RuntimeState` is freed only after every record that names it is cleared. | `ShutdownDrainExecutionCustody`, `close_process_retirement_admission` (refuses while other leases live), the persistent-execution release before the state is dropped. | Differential: `atexit_core_behaviors`, `atexit_exception_unraisable`, `finalizer_exit_semantics`, a thread alive at exit. Unit test: no record names a freed state. |
| J6 free-threaded mode | Record fields are thread-owned (plain cells, read and written by their thread only). Cross-thread facts live in the runtime state's atomic prefix (pending work, recursion limit). Stage 2 adds no cross-thread write into a record. Attachment, not the GIL, defines validity. | Layout in section 2.2. Today the `free-threaded` feature changes refcount atomicity only and keeps the GIL (`lib.rs:24-37`). | `molt-runtime` unit tests with `--features free-threaded`. A GIL-free build is not a cell yet (section 10). |
| J7 error and exception custody | The pending byte is the inline authority for "an exception is pending in the active execution context" (the thread slot, or the installed task's slot: `exceptions/state.rs`). Every backend tests that byte through the context. Attached entries raise and clear through the same token functions as guarded entries. The C-API error slot is unchanged (`with_preserved_error`). | `exception_pending_flag()` writers; task install and uninstall. | Differential: `inline_poll_exception_observation`, `exception_traceback_chain`, generator `throw` and `close`, a task switch with a pending exception, on all three backends. |
| J8 no detach under compiled code | No entry reachable from compiled code ends its own thread's lease, drain custody or GIL custody before it returns. | Detach transitions are boundary functions, not attached entries. Phase B audits `c_api/molt_api.rs:74` (`release_persistent_runtime_execution`). | Hardened assertion at every attached entry exit. |
| J9 GIL depth | An attached entry adds no GIL depth unit. Nothing reads the depth except to test non-zero (`signal_ext.rs:349`, a trace print). | `AttachedContext::token` | Existing GIL unit tests. |

Two open producers belong in review (section 9): whether compiled code runs while the runtime is
`Initializing` (the owner holds no lease: `try_acquire_runtime_execution_lease_kind` returns
`Some(false)`), and whether the finalizing owner runs compiled code under its retained lease or only
under drain custody. If compiled code runs under the `Initializing` owner, that phase is a third
binding producer. Phase B finds out on its first run: `molt_thread_record` fails closed.

## 5. Which entries skip the guard (migration)

### 5.1 Rule

Stage 2 migrates every runtime entry that takes the guard on the hot path of the measured
workloads (section 6), except entries another lane owns. Phase B's first step confirms the set with
`tools/opt_explorer.py anatomy` on those workloads. A confirmed entry that is not in the table below
is added to it, and the change is reported.

### 5.2 Entries

| entry | stage 2 | backends that emit it |
|---|---|---|
| `molt_thread_record` | acquisition: thread-local + attachment check, no state resolution, no store | all three |
| `molt_recursion_enter_fast`, `molt_recursion_exit_fast` | deleted; inline compare of `recursion_depth` with the state's limit, increment and decrement | Cranelift, WASM |
| `molt_raise_recursion_error` | attached, cold | Cranelift, WASM |
| `molt_exception_pending`, `molt_exception_pending_fast` (compiled-code uses) | deleted from compiled code; inline test of the pending byte | LLVM, WASM, Cranelift |
| `molt_async_work_poll_and_exception_pending`, `molt_async_work_poll_and_exception_last_pending` | attached, cold | all three |
| `molt_trace_enter_slot`, `molt_trace_exit` | attached | all three |
| `molt_frame_homes`, `molt_frame_home_load` | attached | all three |
| `molt_frame_context_set` | attached | Cranelift, LLVM |
| `molt_trace_set_line` | attached | all three |
| `molt_module_get_global` | attached | Cranelift, LLVM |
| `molt_function_direct_call_eligible` | attached | Cranelift, WASM |
| `molt_call_func_owned`, `molt_call_inputs_release` | attached | all three |
| `molt_call_indirect_ic` (`dynamic_call`) | attached, if the census confirms it on the path | all three |
| `molt_call_func_fast0`-`3` (direct-call fallbacks) | attached, if the census confirms them on the path | Cranelift |
| `molt_iter_next_unboxed` (every `for` loop; generator steps) | attached, if the census confirms it | all three |

The inline recursion charge is: load `recursion_depth`; load the limit at `attached_state + 8`;
compare; branch to the cold raise; add one; store. That is about 6 instructions, plus one hoisted
load of `attached_state` per invocation. CPython 3.12 needs 4 (`_Py_EnterRecursivePy` decrements
`tstate->py_recursion_remaining`). Section 7 explains the difference.

### 5.3 Not migrated in stage 2, with owner

| entry | why | owner |
|---|---|---|
| `molt_frame_set_col`, `molt_frame_set_line_col`, `molt_frame_set_line`, `molt_guard_builtin_bindings` | They take no guard. Their cost is the `FRAME_STACK` thread-local, which the context does not reach. Moving a frame-stack pointer into the record is interim plumbing that frame records delete. | HF-185 frame records (call anatomy stage 1) |
| `molt_inc_ref_obj`, `molt_dec_ref_obj` | HF-205 replaces them with inline fast paths and a cold call (`inline_refcount.rs`, reserved). The cold call should take the context. | HF-205; the lane that lands second migrates the cold call (section 9) |
| `molt_handle_resolve` | A pure decode with no guard and no context. | call admission (family B, HF-185) |
| the other compiled-code imports (the manifest has 993 rows) | Each keeps its guarded nested path. No entry has both paths. | stage 2b (section 9) |

### 5.4 Consumers that migrate in the same change

The ABI change is one migration. These consumers move together:

- `molt-codegen-abi`: the layout constants; `THREAD_RECORD_PENDING_WORK_PTR_OFFSET` becomes
  `THREAD_RECORD_ATTACHED_STATE_OFFSET`; new `THREAD_RECORD_RECURSION_DEPTH_OFFSET`,
  `RUNTIME_STATE_PENDING_WORK_OFFSET`, `RUNTIME_STATE_RECURSION_LIMIT_OFFSET`.
- The manifest and every generated projection: `wasm_abi_generated/*`, `molt-ir`
  `runtime_boxed_abi_generated.rs` and `runtime_callable_abi_generated.rs`,
  `src/molt/_wasm_abi_generated.py`, `src/molt/_intrinsic_symbols.py`, `intrinsics/manifest.pyi`,
  the new runtime signature module (`tools/generators.py write`).
- `molt-runtime`: `state/tls.rs`, `concurrency/execution.rs` (binding, `AttachedContext`, macros),
  `concurrency/gil.rs` (`PyToken`), `state/runtime_state.rs` (`repr(C)` prefix, `runtime_state`),
  `state/recursion.rs`, each attached entry's definition, and each internal caller (moved to the
  body function).
- Cranelift: the prologue, `fc/calls.rs` (recursion, direct calls), `shared.rs`, the inline
  exception test for functions without an exception label.
- LLVM: `runtime_imports/fixed.rs`, `op_dispatch.rs`, `value_materialization.rs`, call lowering.
- WASM: the import emitters, `exceptions.rs`, `loops.rs`, `branches.rs`, `ops.rs`, `emission.rs`,
  `pending_work.rs`, `call_ops`.
- The JIT's symbol binding and tests (`molt-jit/tests/jit_code_owner.rs`), and
  `molt-backend/tests/native_batch_worker_spawn.rs`.

Not consumers: the Rust and Luau source backends and MLIR (section 2.4); satellite runtime crates,
whose entries keep the guard (HF-206 owns their bridges).

### 5.5 Commit order in Phase B

1. Record layout, attach binding, `AttachedContext`, `PyToken` record, `runtime_state` reads the
   binding. Runtime only; no ABI change. Measurable alone.
2. Manifest field, generator, attached entries and all three backends in one commit, with the
   generated projections in a separate commit beside it (HF-709 commit shape).
3. Inline recursion charge and inline exception tests on all three backends.
4. Measurements, differentials, documentation alignment (section 11).

## 6. Measurement plan

**One runtime product.** Native Cranelift, logical `release` profile, `release-output` runtime,
macOS arm64 (this host). Base: the lane's merge base with `origin/main`, built in its own worktree.
Head: the lane. CPython 3.12.16 (`-I -S`) as the reference. Same host session for both.

**Workloads.**

| id | program | unit |
|---|---|---|
| W1 | `def add(a, b): return a + b`; `total = add(total, i)` for 20,000,000 iterations | call |
| W2 | `def gen(n):` yielding `0..n-1` with a `while` loop; `for v in gen(6_000_000): total += v` | generator step |
| W3 | `python -B tools/perf_scoreboard.py primitives --cpython 3.12.16 --out-dir <dir>`: `recursion` and `dynamic_call` are the targets; `loop`, `id` and every startup axis are guards | per op |
| W4 | `print("hello, world")` | startup |

Stage 1 did not retain its program text (`logs/agents/lane-runtime-entry/artifacts` is empty), so
W1 and W2 are declared here and measured fresh on both sides. Stage-1 numbers are not compared.

**Metrics.**

- Instructions retired and cycles: `/usr/bin/time -l` (`ri_instructions`), as
  [the performance authority](../../tools/PERF_AUTHORITY.md) defines. Per unit:
  `(I(N) - I(0)) / N`, five paired runs, median. W3 uses the scoreboard's own pairing and noise
  bounds.
- Runtime calls per iteration: `tools/opt_explorer.py anatomy --function` on the caller loop and
  the callee; count the call sites on the non-raising path. Cross-check with
  `tools/opt_explorer.py profile` shares. Disassemble each attached entry to show that it has no
  thread-local access and no GIL depth update.
- Dynamic call counts, where available: `perf probe -x <binary>` uprobes on the Linux VM
  (`molt-linux`), which count without a debugger. If the VM run does not happen, that cell is
  reported as unverified.
- Peak RSS, binary size and `__text` size for every program.

**Other backends.**

- LLVM: W1, W2 and the recursion program built with `--backend llvm`, the same counter; anatomy
  shows zero `molt_exception_pending` calls on the hot path.
- WASM: the import census per hot function (zero calls to `molt_exception_pending` and the
  recursion pair) and wall time under the canonical runner. Instruction counts for WASM are an
  unverified cell.

**Acceptance.**

1. Every differential named in section 4 passes on native, LLVM and WASM against CPython 3.12,
   except the cells blocked by HF-689 (LLVM signals) and HF-690 (C extensions), which are reported.
2. `recursion` and `dynamic_call` instructions per op fall. No primitive axis regresses outside its
   ratchet bound. Startup (W4 and the `empty` axis) stays within noise.
3. No attached entry contains a guard or a thread-local access in the release binary.

**Projection** (to test, not to cite). Per Python call on the native recursion primitive: seven
guards at about 42 instructions, about seven `_tlv_get_addr` thunk calls, about three state
resolutions at 21, the recursion pair (about 55, replaced by about 7 inline) and part of the
acquisition: about 400 to 500 of 4,410 instructions. If family B lands first and removes the global read and admission calls from
proven sites, the stage-2 share falls by about 100.

## 7. Prior art and deviations

- **CPython 3.12.** `_PyThreadState_GET` reads one thread-local, `_Py_tss_tstate`
  ([`pycore_pystate.h`](https://github.com/python/cpython/blob/v3.12.0/Include/internal/pycore_pystate.h)).
  Internal functions take `tstate`: `_PyEval_EvalFrameDefault(PyThreadState *tstate, ...)`.
  `_Py_EnterRecursivePy` decrements `tstate->py_recursion_remaining`, and `Py_SetRecursionLimit`
  rebases every thread's counter from the calling thread
  ([`ceval.c`](https://github.com/python/cpython/blob/v3.12.0/Python/ceval.c); 3.13.0 keeps the same
  loop). Molt adopts the explicit context and the one thread-local read.
  **Deviation 1:** Molt reads the limit from the runtime state (two more loads) instead of a
  per-thread remaining counter. The rebase loop is a cross-thread write into thread records, which a
  GIL-free build must serialize; J6 keeps records thread-owned.
- **HotSpot.** The thread lives in a register: `r15_thread` on x86-64, `rthread = r28` and
  `rheapbase = r27` on AArch64
  ([`assembler_aarch64.hpp`](https://github.com/openjdk/jdk/blob/jdk-21-ga/src/hotspot/cpu/aarch64/assembler_aarch64.hpp)).
  Compiled code reads the pending exception and the safepoint poll word at fixed offsets from it.
  Calls into the C++ runtime pass the thread as an ordinary argument.
- **V8.** `kRootRegister` is x26 on arm64
  ([`register-arm64.h`](https://github.com/v8/v8/blob/main/src/codegen/arm64/register-arm64.h)) and
  r13 on x64. Generated code reads `IsolateData` at constant offsets. C++ runtime functions receive
  `Isolate*` as an argument.
- **GraalVM Native Image** (an AOT compiler with isolates). `THREAD_REGISTER = r15` and
  `HEAP_BASE_REGISTER = r14` on AMD64
  ([`AMD64ReservedRegisters.java`](https://github.com/oracle/graal/blob/master/substratevm/src/com.oracle.svm.core.graal.amd64/src/com/oracle/svm/core/graal/amd64/AMD64ReservedRegisters.java)).
  A C entry point takes the context as an explicit parameter: "An execution context must be passed
  as an IsolateThread that is specific to the current thread"
  ([`CEntryPoint`](https://www.graalvm.org/sdk/javadoc/org/graalvm/nativeimage/c/function/CEntryPoint.html)).
  Molt's split is the same: a boundary entry establishes the context; inside, it is carried.
- **OCaml 5** (an AOT native compiler). The domain-state pointer lives in r14 on amd64 and x28 on
  arm64; the allocation pointer in r15 and x27
  ([amd64](https://github.com/ocaml/ocaml/blob/5.2/asmcomp/amd64/proc.ml),
  [arm64](https://github.com/ocaml/ocaml/blob/5.2/asmcomp/arm64/proc.ml)).
- **Swift and LLVM.** `swiftself` is passed in X20, `swifterror` in X21 and `swiftasync` in X22 on
  AArch64
  ([`AArch64CallingConvention.td`](https://github.com/llvm/llvm-project/blob/main/llvm/lib/Target/AArch64/AArch64CallingConvention.td)).
  A context in a callee-saved register survives ordinary calls without moves. Stable Rust has no
  calling convention that assigns a parameter to such a register, so Molt's runtime cannot be the
  callee.
- **Lean 4.** IO functions take a world token, `lean_io_mk_world()` returns `lean_box(0)`, and
  `lean_inc_ref` takes no context
  ([`lean.h`](https://github.com/leanprover/lean4/blob/master/src/include/lean/lean.h)). The token
  is a typed proof that costs one argument and carries no data. `AttachedContext` is the same proof
  with the record address in it.
- **Cranelift and Wasmtime.** Cranelift offers a pinned register (`enable_pinned_reg`;
  `get_pinned_reg`, `set_pinned_reg`): x21 on AArch64, r15 on x86-64
  ([AArch64 `regs.rs`](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/codegen/src/isa/aarch64/inst/regs.rs),
  [x64 `regs.rs`](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/codegen/src/isa/x64/inst/regs.rs)).
  Wasmtime passes `VMContext` as an argument instead (prior art J in
  [findings_prior_art.md](findings_prior_art.md)).

**Deviation 2: no register.** HotSpot, V8, Graal and OCaml own both their code generator and their
runtime stubs. Molt's runtime is Rust compiled by rustc with the C ABI. LLVM would need a reserved
register on each target, and WASM has no registers. An argument costs one move per call; section 2.5 has the numbers.
This matches the earlier refusals (prior art J; call anatomy section 4.11).

**Deviation 3: per-invocation acquisition until HF-670.** CPython passes `tstate` into each frame
evaluation. Stage 2 keeps one `molt_thread_record` call per compiled invocation (about 12
instructions plus the thunk after this change) because HF-670 changes the compiled entry
signature anyway.

**Deviation 4: release builds trust the type.** CPython does not check `tstate` in release builds
either. Molt checks once per invocation in `molt_thread_record` and fails closed.

## 8. Refused alternatives

- **A pinned register** (section 2.5 and deviation 2).
- **Context as a parameter of every compiled entry in stage 2** (decision 6; HF-670 owns it).
- **A frame-stack pointer in the record** (section 5.3; frame records delete the frame stack).
- **A per-token or per-entry state cache.** The binding at attach replaces the cache; nothing is
  revalidated per entry.
- **Two symbols per entry** (a guarded twin and an attached twin). That is the second guard path the
  brief forbids. Runtime-internal callers use body functions.
- **Migrating all 993 manifest rows in stage 2.** It is mechanical but touches about 4,800
  `with_gil_entry*` sites; section 9 asks the integrator.

## 9. Open questions for the integrator

1. **Compiled-to-compiled parameter.** The DAG edge HF-198 → HF-670 says "the native calling
   convention carries the execution context (HF-198 stage 2)". This design gives HF-670 the
   context and the attached ABI, and leaves the compiled entry parameter to HF-670. Confirm, or
   move the parameter into this lane.
2. **Stage 2b.** After stage 2, about 950 compiled-code imports keep the guarded nested path. I
   recommend a separate mechanical sweep with its own ledger row once stage 2 has measured the
   per-entry gain. The manifest field makes it a row-by-row change. Alternatively Phase B does all
   rows now.
3. **HF-205 order.** HF-205's cold refcount call (`slow_call` in `inline_refcount.rs`, reserved for
   slot-10) should take the context. Which lane lands second and migrates it?
4. **Family B overlap.** Slot-05 rewrites `handle_call_guarded_op`, `molt_module_get_global` and
   admission at proven call sites. Both lanes touch `fc/calls.rs`. Name the merge order.
5. **`Initializing` owner.** If compiled code runs during runtime initialization, the initializing
   owner is a third binding producer (section 4). I will find out in Phase B; approve that
   extension in advance or ask me to return.
6. **`RuntimeState` as `repr(C)`.** The hot prefix needs a fixed layout. The alternative is a
   separate `repr(C)` block and a second record pointer. I chose `repr(C)` for one pointer and one
   fact; it costs padding in one allocation per runtime.
7. **Rename.** `ExecutionTls` (storage name) and "thread record" (ABI name) name one thing. Rename
   the Rust type `ThreadRecord` in Phase B (eight files), or keep both names.

## 10. Risks and unverified cells

- The binding at attach touches lifecycle code (`runtime_state.rs`, `execution.rs`) whose
  lifetimes the convergence document lists as distinct: GIL, attachment, isolate, shutdown worker,
  TLS destruction. J2 and J5 are the highest-risk invariants. Their unit tests come before the ABI
  change (commit 1 in section 5.5).
- A GIL-free mode does not exist. The `free-threaded` feature keeps the GIL. J6 states the layout
  rule; nothing proves a GIL-free attach protocol.
- C-extension execution is blocked on main by HF-690; LLVM signal execution by HF-689. WASM threads
  and OS signals are unsupported cells. Linux and x86-64 counts are unverified unless the VM run in
  section 6 happens.
- The per-call projection comes from static instruction counts in one binary, which the HF-657
  lane built on 2026-11-02 from its own head, not from this lane's base. Section 6 replaces it with measurements.
- Phase B conflicts are likely with HF-653 (slot-06), which edits many runtime files, and with
  family B (slot-05) in the native call emitters.

## 11. Documents to align on approval

- `docs/design/foundation/call_anatomy.md`: section 4.1 and stage 3 point here for the context
  representation; stage 3 keeps the compiled entry parameter (open question 1).
- `docs/design/findings_prior_art.md`, family J: add the Graal, OCaml, Swift and Lean entries above.
- `docs/design/foundation/77_architecture_convergence.md`, section C: link this design.
- `docs/INDEX.md`: list this design.
- The HF-198 ledger row (integrator).
