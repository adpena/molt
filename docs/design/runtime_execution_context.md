# Runtime execution context (HF-198 stage 2)

Status: revised 2026-11-03 after review. The first draft proposed an attached ABI: runtime entries
that take the execution context as an argument. Three reviews rejected it as written (section 6).
Stage 2 is now three changes and a measurement gate: B1 makes the nested runtime entry nearly free
and binds the runtime state once per attachment; B2 tests the pending-exception byte inline on LLVM
and WASM; the gate measures both. The attached ABI stays a recorded, deferred option. Owner: lane
`hf198-entry-context`; review and landing: integrator. Ledger: HF-198. Coordinates with HF-670
(calling convention), HF-205 (inline refcount), HF-185 (frame records) and HF-657 (eval-breaker
word).

This design refines [call anatomy section 4.1](foundation/call_anatomy.md) and
[architecture convergence section C](foundation/77_architecture_convergence.md); section 10 lists
the lines that change there.

## 0. Decision

1. **B1: the nested entry borrows.** A runtime entry whose thread already holds admission custody
   (a lease or shutdown-drain custody) and the GIL takes no GIL depth unit, builds no `GilGuard`,
   and does nothing on drop. Every other entry takes today's cold path unchanged. No entry aborts
   for lack of a binding. This helps all 4,811 `with_gil_entry*` sites, not only the call path.
2. **B1: the runtime state is bound once per attachment.** `ExecutionTls` gains `bound_state`.
   The lease 0 → non-zero edge and the drain 0 → 1 edge write it; the matching release edges, a
   thread state switch and every unpublish of the ready pointer reset it. `runtime_state(py)`
   reads it with one load and falls back to today's full resolution when it is null. This deletes
   the per-token state cache (`03778a0ca`).
3. **B2: the exception byte is tested inline.** Every LLVM and WASM exception check that runs in a
   function that already holds the thread record loads the record's pending byte. It calls
   `molt_exception_pending` only when the byte is set, so that reader keeps its repair behavior.
   Cranelift already tests the byte inline.
4. **The attached ABI is deferred.** Section 6 keeps its design, its costs and the review blockers
   that any later attached ABI must answer. The integrator decides after the gate whether it is
   worth a new row.

## 1. What exists today

### 1.1 Compiled code already holds the thread record

Stage 1 (`5041f8cd05`) put the lease, GIL, drain and runtime-state facts in one const-initialized
thread-local with no destructor. `67ccbbb110` made that struct the thread record compiled code
holds; HF-657 (`465726e3da`) added the pending-work pointer. `molt-codegen-abi` declares the ABI
prefix (`THREAD_RECORD_*`), and `state/tls.rs` asserts it with `offset_of!`:

| offset | field | written by | read by |
|---:|---|---|---|
| 0 | `exception_pending: Cell<bool>` | raise, clear, task switch | the runtime; Cranelift inline checks; WASM and LLVM inline polls |
| 8 | `pending_invocation: Cell<u64>` | direct-call handoff (inline stores) | the callee's `molt_trace_enter_slot` |
| 16 | `pending_work_ptr: Cell<u64>` | `molt_thread_record`, each invocation | inline polls, all backends |

`pending_work_ptr` is the address of the word inside `Arc<PendingWork>`. The `RuntimeState`, its
GC state, its concurrent state and its signal state share that `Arc` (`runtime_state.rs:539-636`).
It is not a field at a fixed offset of `RuntimeState`.

Who takes the record, and when:

- **Cranelift** calls `molt_thread_record` in the entry block of a function with inline exception
  checks, direct Python calls or async polls, and keeps the result in a stack slot
  (`function_compiler.rs:493-537`, `:664-679`).
- **LLVM** materializes it in the entry block on first use, today only for async polls
  (`async_poll_context`, `llvm_backend/lowering/value_materialization.rs:1821`).
- **WASM** calls the `thread_record` import in the prologue of a function with async polls
  (`emit_entry_initializers`, `wasm/function_frame/emission.rs:149`) and keeps it in the
  `__thread_record` local. The `lir_fast` lane has no such local.

### 1.2 What one runtime entry costs

Static counts from the release binary the HF-657 lane built and retained on 2026-11-02
(`logs/agents/lane-hf657-inline-poll/artifacts/anatomy-after/loop_molt`, macOS arm64,
`objdump -d`). The `_tlv_get_addr` thunk is not counted.

| code | non-raising path |
|---|---|
| Guard in a nested entry (`molt_exception_pending_fast`) | 42 instructions outside the body: 4 to reach the `_tlv_get_addr` thunk, 5 to test lease and drain depth, 6 to raise the GIL depth, 12 to build the guard and token on the stack, 15 to drop them |
| `runtime_state_for_gil_in` (main thread, Ready) | 21 instructions, out of line; a token runs it on its first `runtime_state` call |
| `molt_thread_record` | 17 instructions, one thunk call, and `runtime_state_for_gil_in` |
| `molt_recursion_enter_fast` | 20 instructions, two thread-local accesses, and `runtime_state_for_gil_in` |

B1 removes the depth update and the guard construction from every nested entry and turns the
state resolution into one load. Measured on the same entry after B1 (section 5): 21 instructions
outside the body instead of 42: 4 to reach the thunk, 6 for the custody test (now including the
GIL depth), 5 to build the token, and 6 for the drop's ownership test, which the compiler keeps
because the cold path joins the same drop.

### 1.3 The entries one Python call makes (native)

| step | entry | today |
|---|---|---|
| callee prologue | `molt_thread_record` | thread-local + state resolution |
| frame push, homes, argument zero, frame pop | `molt_trace_enter_slot`, `molt_frame_homes`, `molt_frame_context_set`, `molt_trace_exit` | guard each |
| line, column | `molt_frame_set_line_col`, `molt_frame_set_col` | `FRAME_STACK` thread-local, no guard |
| caller: global read, admission | `molt_module_get_global`, `molt_function_direct_call_eligible` | guard each |
| caller: decode | `molt_handle_resolve` | no guard |
| caller: recursion | `molt_recursion_enter_fast`, `molt_recursion_exit_fast` | thread-locals + state resolution |
| caller: poll | inline word test (HF-657); cold `molt_async_work_poll_and_exception_pending` | guard on the cold path |
| both: counts | `molt_inc_ref_obj`, `molt_dec_ref_obj` | guard for heap objects |

Seven entries take the full guard on every call. Call-anatomy stages 1-3 delete five of them
(frame records delete the frame entries; compile-time binding deletes the global read and the
admission call), which is one reason the review moved the saving to the guard itself.

### 1.4 Backend gap: the exception test

Cranelift tests the record's pending byte inline in functions with an exception label. LLVM calls
`molt_exception_pending` for every `check_exception`, `ExceptionPending` and borrowed-operand
continuation (`op_dispatch.rs:458`, `:520`; `value_materialization.rs:325`). WASM calls it from
seven emitters (section 4). Each call runs the guard through `with_raw_gil_entry_nopanic!`.

## 2. B1: the nested entry borrows

### 2.1 Predicate

The nested branch keeps today's admission predicate and drops only the work after it:

| target | nested when | today's branch (`execution.rs:543-583`) |
|---|---|---|
| native | `(lease_depth != 0 or shutdown_drain_depth != 0) and gil_depth != 0` | same test, then a `GilGuard` that raises `gil_depth` |
| wasm32 | `current_thread_has_runtime_execution_custody`: drain custody, or a lease and a non-zero `WASM_RUNTIME_EXECUTION_DEPTH` | lease or drain, then a no-op `GilGuard` |

`gil_depth` is never written on wasm32, so the wasm32 predicate uses the cfg-aware custody test,
never `gil_depth`.

### 2.2 Representation

```rust
pub(crate) struct RuntimeExecutionGuard {
    tls: &'static ExecutionTls,
    /// `None`: the entry borrowed the custody and GIL its thread held.
    owned: Option<OwnedExecution>,
}
struct OwnedExecution {
    gil: GilGuard,
    custody: Option<RuntimeExecutionCustody>,
    active_lifecycle_lease: bool,
}
```

- **Entry.** The inline part reads the thread-local once, tests the predicate and returns
  `owned: None`. Otherwise it calls the cold path.
- **Cold path.** It is today's code with one addition at its top: when the thread holds custody but
  the predicate failed, it first tries `GilGuard::new_if_held_in`. On native that reaches only the
  TLS-destruction GIL lane (`gil_depth` is zero there); on wasm32 it is today's no-op guard. Both
  return the guard today's inline branch returned, with no custody. The four destruction lanes
  (`ThreadLocalGuard::drop`, `ThreadExceptionRelease::drop`, `enter_retained_thread_state_drop`,
  `GilGuard::tls_destruction_new`) therefore see exactly today's guards.
- **Drop.** A borrowed entry does nothing. An owned entry finishes its custody and releases its
  GIL guard as today.
- **Token.** `PyToken` holds `&'static ExecutionTls` instead of `&GilGuard`, plus a lifetime marker.
  `GilGuard::token` and `RuntimeExecutionGuard::token` build the same type. The token stays
  `!Send` (the record holds `Cell`s).
- **Encoded lane.** `into_encoded_lane` gives a borrowed entry its own code (`1 << 13`, outside
  the GIL lane and custody bits); `from_encoded_lane` rebuilds a borrowed entry from it. The WASM
  host boundary and the core GIL vtable keep working for nested callers.

### 2.3 Why the borrow is sound

- An owned entry above it on the same thread holds the GIL until it drops, and it drops after the
  borrowed entry returns (stack order).
- A GIL release inside the body (`GilReleaseGuard::suspend`, `PyEval_SaveThread`) saves and
  restores the whole depth (`gil.rs:452`), so it needs no unit from the borrowed entry.
- Nothing reads `gil_depth` except to test it for zero (`gil_held`, `signal_ext.rs:349`) or to
  print it.
- Unchanged hazard: the public `molt_gil_release` can end the persistent attachment while a runtime
  entry is still running. Today the nested depth unit keeps the mutex locked until that entry
  returns; with the borrow, the mutex unlocks at once. Either way the code after it runs detached,
  so this is a misuse of the embedding API in both designs, not a new state.

### 2.4 The bound runtime state

`ExecutionTls::bound_state: Cell<*const RuntimeState>` is null or the state this thread's
current attachment runs.

| edge | owner | action |
|---|---|---|
| lease 0 → non-zero (ordinary or service) | `try_acquire_runtime_execution_lease_kind` (`runtime_state.rs:1127`) | bind to `runtime_state_for_gil_in` after the Ready recheck |
| drain 0 → 1 | `ShutdownDrainExecutionCustody::enter` | bind |
| lease non-zero → 0, drain 1 → 0 | `release_runtime_execution_lease`, the drain custody drop | reset to null when both are zero |
| `set_thread_runtime_state` | `runtime_state.rs` | rebind when the thread holds custody |
| `clear_thread_runtime_state` | `runtime_state.rs` | reset to null |
| ready pointer unpublished (shutdown, process exit, lifecycle failure) | `runtime_state.rs` | reset the current thread to null |

- **Retained workers.** `for_retained_worker` records `retained_worker_origin` before the drain
  edge binds. Today it records it after `enter` raises the drain depth, and during `Finalizing`
  the resolution needs that origin.
- **Why the binding stays equal to the resolution.** The resolution reads the service-role bit,
  the retained origin, `tls.runtime_state`, the ready pointer and the lifecycle phase. The edges
  above cover every change to them while a thread holds custody: the service bit and the origin
  change only at custody edges; `tls.runtime_state` changes only through the two functions above;
  the ready pointer is published only by initialization (whose owner holds no lease) and
  unpublished only at the resets above; and retirement refuses while another ordinary lease lives
  (`close_process_retirement_admission`).
- **Null is always safe.** A null binding takes today's full resolution, so any edge this list
  misses costs speed, not correctness.
- **Readers.** `runtime_state(py)`, `runtime_state_for_gil()` and `molt_thread_record` read the
  binding first. `runtime_state_for_gil_in` stays the full resolution that the edges use.
- **Initialization.** The `Initializing` owner holds no lease (`Some(false)`), so it never binds;
  no compiled code runs then.

## 3. Invariants and their tests (B1)

| id | invariant | test |
|---|---|---|
| N1 | A nested entry needs the same custody as today's inline branch, adds no GIL depth unit and owns nothing; its encoded lane round-trips. | `nested_entry_borrows_custody_without_a_depth_unit`; the existing drain-custody tests now assert `nested.owned.is_none()`. |
| N2 | The four destruction lanes see today's guards and run finalizers. | The cold path keeps today's guard for them (section 2.2); the existing `molt-runtime` TLS-destruction, thread-exception-release, retained-worker and isolate-teardown tests. |
| N3 | `bound_state` equals the full resolution whenever it is non-null. | Debug builds assert it at every read while the ready state is published, so every runtime test checks it. `lease_custody_binds_the_resolved_state_until_its_release`, `a_runtime_switch_under_custody_rebinds_and_a_clear_unbinds`, `drain_custody_binds_and_its_drop_unbinds`; the isolate-switch tests (`signal_ext.rs:2063-2069`, `state/recursion/tests.rs:94`). |
| N4 | A freed or unpublished state is never bound. | `clear_thread_runtime_state` and every ready-pointer unpublish reset the binding; the existing shutdown, process-exit and restart tests. |

## 4. B2: the exception byte inline

For a function that holds the thread record, every exception check becomes:

```text
byte = load u8 [record + THREAD_RECORD_EXCEPTION_PENDING_OFFSET]
pending = byte != 0 ? molt_exception_pending() : 0
```

`exception_pending` returns false at once when the byte is clear (`exceptions.rs:2262`), so the
inline test returns exactly what the call returned. When the byte is set, the call confirms it and
keeps its repair: a stale task slot clears the byte and reports no exception. The non-raising path
costs one load and one branch.

| backend | emitters | record source |
|---|---|---|
| LLVM | `check_exception` (`op_dispatch.rs:520`), `ExceptionPending` (`:458`), borrowed-operand continuation (`value_materialization.rs:325`) | `async_poll_context`, in functions that contain an async poll (the record is already paid for) |
| WASM | `control_ops/exceptions.rs:77`, `control_ops/loops.rs:69`, `state_dispatch/common/branches.rs:97`, `state_dispatch/emit/ops.rs:112`, `core_runtime_ops/runtime_effect_ops.rs:47`, `aggregate_ops/hash_container_ops.rs:69`, `function_frame/emission.rs:119` | the `__thread_record` local, when the function has it |

- **Kept as a call:** WASM `lir_fast` (`lir_fast/lir_ops.rs`, `lir_runtime_ops/exception_ops.rs`,
  `lir_context/ownership.rs`) has no record local; functions without async polls on both backends
  have no record, and acquiring one for a single check would cost more than the call.
- **Not added:** LLVM emits no recursion charge today, and this change does not add one.
- **Cranelift** is unchanged; it already tests the byte inline where it holds the record.

## 5. Measurement gate

One runtime product: native Cranelift, logical `release` profile, `release-output` runtime, macOS
arm64. Base: the lane's base built and measured in this checkout before any runtime change. Head:
the lane after B1 and B2, same environment.

| id | workload | metric |
|---|---|---|
| W1 | `def add(a, b): return a + b`; `total = add(total, i)` for 20,000,000 iterations | instructions and cycles per call |
| W2 | a `while` generator yielding `0..n-1`; `for v in gen(6_000_000): total += v` | per generator step |
| W3 | `tools/perf_scoreboard.py primitives --cpython 3.12.16`: `recursion` and `dynamic_call`, with every other axis as a guard | per op, against CPython 3.12.16 |
| W4 | `print("hello, world")` | startup instructions |

- **Counter:** `/usr/bin/time -l` instructions retired ([performance authority](../../tools/PERF_AUTHORITY.md)),
  `(I(N) - I(0)) / N`, five runs, median.
- **Runtime entries per call:** `tools/opt_explorer.py anatomy` on the W1 callee and caller loop
  (unchanged by B1, which removes cost inside entries, not entries), plus the disassembly of one
  nested entry before and after.
- **LLVM and WASM:** the primitive scoreboard is native-only. For them the gate reports the static
  emitter census (calls to `molt_exception_pending` on the non-raising path) and the backend test
  suites.
- **Residual guard cost:** the instructions a nested entry still spends on custody (thread-local
  access and predicate), read from the head disassembly.

**Result** (2026-11-03; base `f0244aaaae`, head `821ca692c2`; instructions retired, medians of five
runs; CPython 3.12.16 for the ratios):

| workload | base | head | change |
|---|---:|---:|---:|
| W1, per call | 4,401.8 | 4,003.0 | -9.06% |
| W2, per generator step | 6,707.2 | 6,233.5 | -7.06% |
| W3 `recursion`, per depth-100 traversal | 441,377.6 (7.83x) | 400,093.3 (7.09x) | -9.35% |
| W3 `dynamic_call`, per op | 2,957.4 (3.46x) | 2,648.6 (3.10x) | -10.44% |
| W3 `loop`, per op | 392.9 | 373.8 | -4.88% |
| W4 startup | 191.75 M | 191.37 M | -0.20% |

Every other primitive per-op axis fell by 4.7% to 9.3%; no per-op axis rose. Startup axes moved -1.6% to
+0.1%. The W1 binary is 0.37% smaller. Runtime entries per call are unchanged (B1 removes cost
inside entries); the residual guard is 21 instructions plus one `_tlv_get_addr` thunk call per
nested entry.

## 6. Deferred: the attached ABI

The first draft proposed that compiled code pass the thread record as the first argument of the
call-path runtime entries (`AttachedContext<'a>`, declared per manifest row), skipping the guard
entirely. It also proposed inline recursion charges, a `repr(C)` `RuntimeState` prefix, and a
compiled-to-compiled context parameter after HF-670. The design and its cost table remain in this
file's history at `f0244aaaae`. The review found these blockers, which any later attached ABI must
answer:

- **Destruction lanes run Python with no attach transition:** `ThreadLocalGuard::drop`,
  `ThreadExceptionRelease::drop`, `enter_retained_thread_state_drop`,
  `GilGuard::tls_destruction_new`. An entry that requires a binding would abort there.
- **`gil_depth` is never written on wasm32**, so it cannot prove attachment there.
- **The public `molt_gil_release` can end an attachment mid-invocation.**
- **Context forgery:** Python-callable intrinsics and `*const ()` resolver casts can reach an
  attached entry without a real context.
- **The ABI fingerprint does not cover a signature change.**
- **`pending_work` is an `Arc` shared by three sub-states**, not a `RuntimeState` prefix word.
- **Five of the seven per-call guarded entries are deleted by call-anatomy stages 1-3 anyway.**

The draft also had wrong claims, corrected here: the `pending_work` layout (section 1.1); the
wasm32 GIL (section 2.1); its backend table, which treated LLVM as holding the record in every
function (section 1.1); its WASM emitter list (section 4); and its process-wake row. Process wake
installs the origin with `set_thread_runtime_state` and enters through
`RuntimeExecutionGuard::try_ready` (`async_rt/process.rs:1582`), not the service guard.

Choices the draft made that stand: the context is the thread record (not a second structure or the
runtime state); a pinned register is refused, because the Rust runtime cannot read one and LLVM and
WASM cannot carry one; the compiled-to-compiled context parameter belongs to HF-670.

## 7. Prior art

- **CPython 3.12.** `_PyThreadState_GET` reads one thread-local, `_Py_tss_tstate`
  ([`pycore_pystate.h`](https://github.com/python/cpython/blob/v3.12.0/Include/internal/pycore_pystate.h)),
  and internal functions take `tstate`. A C-API call assumes the caller holds the GIL; it does no
  per-call admission (only the debug build checks `tstate`, with `_Py_EnsureTstateNotNULL` in
  `_PyInterpreterState_GET`). B1 moves Molt's nested entry toward that cost: a custody test and no
  bookkeeping. `tstate->interp` is one load, as `bound_state` is.
- **HotSpot and V8.** The thread (`r15_thread`; `rthread = r28` on AArch64,
  [`assembler_aarch64.hpp`](https://github.com/openjdk/jdk/blob/jdk-21-ga/src/hotspot/cpu/aarch64/assembler_aarch64.hpp))
  or isolate root (`kRootRegister`, x26 on arm64,
  [`register-arm64.h`](https://github.com/v8/v8/blob/main/src/codegen/arm64/register-arm64.h)) lives
  in a register. Compiled code reads the pending exception at a fixed offset, as B2 does on every
  backend now.
- **GraalVM Native Image** (`THREAD_REGISTER = r15`,
  [`AMD64ReservedRegisters.java`](https://github.com/oracle/graal/blob/master/substratevm/src/com.oracle.svm.core.graal.amd64/src/com/oracle/svm/core/graal/amd64/AMD64ReservedRegisters.java))
  and **OCaml 5** (domain state in r14 or x28,
  [amd64](https://github.com/ocaml/ocaml/blob/5.2/asmcomp/amd64/proc.ml),
  [arm64](https://github.com/ocaml/ocaml/blob/5.2/asmcomp/arm64/proc.ml)) carry the context in a
  register inside compiled code; Graal's C entry points take it as an explicit parameter
  ([`CEntryPoint`](https://www.graalvm.org/sdk/javadoc/org/graalvm/nativeimage/c/function/CEntryPoint.html)).
  These inform the deferred attached ABI.
- **Swift, Lean, Cranelift.** `swiftself`/`swiftasync` use callee-saved registers
  ([`AArch64CallingConvention.td`](https://github.com/llvm/llvm-project/blob/main/llvm/lib/Target/AArch64/AArch64CallingConvention.td));
  Lean's IO world token is a typed proof with no data
  ([`lean.h`](https://github.com/leanprover/lean4/blob/master/src/include/lean/lean.h)); Cranelift's
  pinned register is x21 on AArch64 and r15 on x86-64
  ([`regs.rs`](https://github.com/bytecodealliance/wasmtime/blob/main/cranelift/codegen/src/isa/aarch64/inst/regs.rs)).
  Stable Rust has no calling convention that puts a parameter in a callee-saved register, so the
  runtime side cannot adopt these.

## 8. Review answers (integrator, 2026-11-03)

1. The compiled-to-compiled context parameter stays with HF-670.
2. No stage-2b sweep.
3. The HF-205 cold refcount call stays as it is.
4. This scope barely touches `fc/calls.rs`; it does not wait for family B.
5. No `Initializing` extension: no compiled code runs then.
6. No `repr(C)` change to `RuntimeState`.
7. No rename of `ExecutionTls`.

## 9. Risks and unverified cells

- The binding touches lifecycle code whose lifetimes the convergence document lists as distinct:
  GIL, attachment, isolate, shutdown worker, TLS destruction. Null is the safe default (section
  2.4), so a missed edge costs speed only.
- C-extension execution is blocked on main by HF-690 and LLVM signal execution by HF-689. WASM
  threads and OS signals are unsupported cells. Linux and x86-64 counts are unverified.
- The gate measures native only; LLVM and WASM report static evidence.

## 10. Documents to align

- `docs/design/foundation/call_anatomy.md`: section 4.1 points here; the attached ABI is deferred.
- `docs/design/findings_prior_art.md`, family J: the Graal, OCaml, Swift and Lean entries above.
- `docs/design/foundation/77_architecture_convergence.md`, section C: link this design.
- `docs/INDEX.md`: list this design.
- The HF-198 ledger row (integrator).
